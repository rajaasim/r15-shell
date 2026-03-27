use std::collections::HashSet;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, COOKIE, REFERER, USER_AGENT};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub message_id: u64,
    pub user_id: u64,
    pub user_name: String,
    pub text: String,
    pub timestamp: String,
}

#[derive(Debug, Clone)]
pub struct ChatClient {
    base_url: String,
    room_id: u64,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct SendMessageResponse {
    id: Option<u64>,
}

impl ChatClient {
    pub fn new(base_url: String, room_id: u64) -> Result<Self> {
        let client = Client::builder()
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            room_id,
            client,
        })
    }

    pub fn fetch_recent_messages(&self, limit: usize, cookie_header: Option<&str>) -> Result<Vec<ChatMessage>> {
        let transcript_url = format!("{}/transcript/{}", self.base_url, self.room_id);
        let body = self
            .client
            .get(&transcript_url)
            .headers(self.request_headers(cookie_header, None)?)
            .send()
            .context("failed to fetch transcript")?
            .error_for_status()
            .context("transcript request failed")?
            .text()
            .context("failed to read transcript body")?;

        Ok(parse_transcript_html(&body, limit))
    }

    pub fn send_message(&self, text: &str, cookie_header: &str) -> Result<u64> {
        let room_url = format!("{}/rooms/{}", self.base_url, self.room_id);
        let fkey = self.fetch_room_fkey(cookie_header)?;
        self.acknowledge_room_rules(cookie_header, &fkey)?;

        let response = self
            .client
            .post(format!("{}/chats/{}/messages/new", self.base_url, self.room_id))
            .headers(self.request_headers(Some(cookie_header), Some(&room_url))?)
            .form(&[("text", text), ("fkey", fkey.as_str())])
            .send()
            .context("failed to send message")?;

        if response.headers().get("x-chat-human").and_then(|value| value.to_str().ok()) == Some("required") {
            bail!("human verification is required for this chat session");
        }

        let payload: SendMessageResponse = response
            .error_for_status()
            .context("message post failed")?
            .json()
            .context("failed to decode send message response")?;

        payload.id.ok_or_else(|| anyhow!("message post succeeded but no message id was returned"))
    }

    fn fetch_room_fkey(&self, cookie_header: &str) -> Result<String> {
        let room_url = format!("{}/rooms/{}", self.base_url, self.room_id);
        let body = self
            .client
            .get(&room_url)
            .headers(self.request_headers(Some(cookie_header), None)?)
            .send()
            .context("failed to fetch room page")?
            .error_for_status()
            .context("room page request failed")?
            .text()
            .context("failed to read room page body")?;

        parse_fkey(&body).ok_or_else(|| anyhow!("chat fkey was not found on the room page"))
    }

    fn acknowledge_room_rules(&self, cookie_header: &str, fkey: &str) -> Result<()> {
        let room_url = format!("{}/rooms/{}", self.base_url, self.room_id);
        let headers = self.request_headers(Some(cookie_header), Some(&room_url))?;

        self.client
            .post(format!("{}/users/set-pref/53", self.base_url))
            .headers(headers.clone())
            .form(&[("fkey", fkey)])
            .send()
            .context("failed to acknowledge chat preferences")?
            .error_for_status()
            .context("chat preference acknowledgement failed")?;

        self.client
            .post(format!("{}/rooms/set-has-seen-guidelines", self.base_url))
            .headers(headers)
            .form(&[("fkey", fkey), ("roomId", &self.room_id.to_string())])
            .send()
            .context("failed to acknowledge room guidelines")?
            .error_for_status()
            .context("room guideline acknowledgement failed")?;

        Ok(())
    }

    fn request_headers(&self, cookie_header: Option<&str>, referer: Option<&str>) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("r15-shell/0.1 (+https://github.com/rajaasim/r15-shell)"),
        );

        if let Some(cookie) = cookie_header.filter(|value| !value.trim().is_empty()) {
            headers.insert(
                COOKIE,
                HeaderValue::from_str(cookie).context("cookie header contains invalid characters")?,
            );
        }

        if let Some(url) = referer {
            headers.insert(
                REFERER,
                HeaderValue::from_str(url).context("referer header contains invalid characters")?,
            );
        }

        Ok(headers)
    }
}

pub fn merge_seen_ids(messages: &[ChatMessage]) -> HashSet<u64> {
    messages
        .iter()
        .filter_map(|message| (message.message_id > 0).then_some(message.message_id))
        .collect()
}

pub fn parse_transcript_html(html_text: &str, limit: usize) -> Vec<ChatMessage> {
    let document = Html::parse_document(html_text);
    let transcript_day = parse_transcript_day(&document);

    let monologue_selector = selector(".monologue");
    let message_selector = selector(".message");
    let signature_user_link_selector = selector(".signature .username a");
    let signature_user_selector = selector(".signature .username");
    let message_user_link_selector = selector(".username a");
    let message_user_selector = selector(".username");
    let timestamp_selector = selector(".messages > .timestamp");
    let inline_timestamp_selector = selector(".timestamp, .times");
    let content_selector = selector(".content");

    let mut messages = Vec::new();

    for monologue in document.select(&monologue_selector) {
        let user_id = parse_monologue_user_id(&monologue);
        let user_name = monologue
            .select(&signature_user_link_selector)
            .next()
            .map(text_of)
            .or_else(|| monologue.select(&signature_user_selector).next().map(text_of))
            .unwrap_or_else(|| "unknown".to_string());

        let monologue_timestamp = monologue.select(&timestamp_selector).next().map(text_of);

        for message_el in monologue.select(&message_selector) {
            let text_container = match message_el.select(&content_selector).next() {
                Some(value) => value,
                None => continue,
            };

            let text = extract_transcript_text(&text_container);
            if text.is_empty() {
                continue;
            }

            let fallback_user_name = message_el
                .select(&message_user_link_selector)
                .next()
                .map(text_of)
                .or_else(|| message_el.select(&message_user_selector).next().map(text_of))
                .unwrap_or_else(|| user_name.clone());

            let timestamp_text = message_el
                .select(&inline_timestamp_selector)
                .next()
                .map(text_of)
                .or_else(|| monologue_timestamp.clone())
                .unwrap_or_default();

            messages.push(ChatMessage {
                message_id: parse_message_id(&message_el),
                user_id,
                user_name: fallback_user_name,
                text,
                timestamp: normalize_timestamp(&timestamp_text, transcript_day),
            });
        }
    }

    if messages.len() > limit {
        messages.split_off(messages.len() - limit)
    } else {
        messages
    }
}

fn parse_fkey(html_text: &str) -> Option<String> {
    let document = Html::parse_document(html_text);
    let input_selector = selector("input#fkey");
    let input = document.select(&input_selector).next()?;
    input.value().attr("value").map(ToOwned::to_owned)
}

fn selector(raw: &str) -> Selector {
    Selector::parse(raw).expect("selector should be valid")
}

fn text_of(element: ElementRef<'_>) -> String {
    normalize_message_text(&element.text().collect::<Vec<_>>().join(" "))
}

fn extract_transcript_text(text_container: &ElementRef<'_>) -> String {
    let mut pieces = text_container
        .text()
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if let Some(first) = pieces.first() {
        let ago_regex = Regex::new(r"^\d+\s+(seconds?|minutes?|hours?|days?)\s+ago$").expect("regex should compile");
        if ago_regex.is_match(first) {
            pieces.remove(0);
        }
    }

    normalize_message_text(&pieces.join(" "))
}

fn normalize_message_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_message_id(message_el: &ElementRef<'_>) -> u64 {
    let id_regex = Regex::new(r"(\d+)").expect("regex should compile");

    for candidate in [
        message_el.value().attr("id"),
        message_el.value().attr("data-messageid"),
    ] {
        if let Some(raw) = candidate {
            if let Some(captures) = id_regex.captures(raw) {
                if let Some(matched) = captures.get(1) {
                    if let Ok(id) = matched.as_str().parse::<u64>() {
                        return id;
                    }
                }
            }
        }
    }

    let link_selector = selector("a[href*='/transcript/message/']");
    if let Some(link) = message_el.select(&link_selector).next() {
        if let Some(href) = link.value().attr("href") {
            let href_regex = Regex::new(r"/transcript/message/(\d+)").expect("regex should compile");
            if let Some(captures) = href_regex.captures(href) {
                if let Some(matched) = captures.get(1) {
                    if let Ok(id) = matched.as_str().parse::<u64>() {
                        return id;
                    }
                }
            }
        }
    }

    0
}

fn parse_monologue_user_id(monologue: &ElementRef<'_>) -> u64 {
    let user_regex = Regex::new(r"user-(\d+)").expect("regex should compile");
    for class_name in monologue.value().classes() {
        if let Some(captures) = user_regex.captures(class_name) {
            if let Some(matched) = captures.get(1) {
                if let Ok(id) = matched.as_str().parse::<u64>() {
                    return id;
                }
            }
        }
    }
    0
}

fn parse_transcript_day(document: &Html) -> Option<NaiveDate> {
    let title_selector = selector("title");
    let title = document.select(&title_selector).next().map(text_of)?;
    let day_regex = Regex::new(r"(\d{4})-(\d{2})-(\d{2})").expect("regex should compile");
    let captures = day_regex.captures(&title)?;

    let year = captures.get(1)?.as_str().parse::<i32>().ok()?;
    let month = captures.get(2)?.as_str().parse::<u32>().ok()?;
    let day = captures.get(3)?.as_str().parse::<u32>().ok()?;

    NaiveDate::from_ymd_opt(year, month, day)
}

fn normalize_timestamp(raw_time: &str, transcript_day: Option<NaiveDate>) -> String {
    let trimmed = raw_time.trim();
    if trimmed.is_empty() {
        return Utc::now().to_rfc3339();
    }

    let day = transcript_day.unwrap_or_else(|| Utc::now().date_naive());
    for pattern in ["%H:%M", "%H:%M:%S"] {
        if let Ok(time) = NaiveTime::parse_from_str(trimmed, pattern) {
            let naive = day.and_time(time);
            let timestamp = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
            return timestamp.to_rfc3339();
        }
    }

    Utc::now().to_rfc3339()
}
