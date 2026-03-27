use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;
use std::{collections::HashMap, collections::HashSet};

use anyhow::{bail, Context, Result};

use crate::chat::{merge_seen_ids, ChatClient, ChatMessage};
use crate::config::Args;

pub fn run(args: Args) -> Result<()> {
    let cookie_file = args.cookie_file_path();
    ensure_parent_dir(&cookie_file)?;

    let initial_cookie = load_cookie(&cookie_file)?;
    let cookie = Arc::new(RwLock::new(initial_cookie));
    let seen_ids = Arc::new(Mutex::new(Default::default()));
    let running = Arc::new(AtomicBool::new(true));

    let chat_client = ChatClient::new(args.chat_url.clone(), args.room_id)?;

    println!("r15-shell");
    println!("room: {}", args.room_id);
    println!("poll: {}s", args.poll_seconds);
    println!("cookie file: {}", cookie_file.display());

    match cookie.read().expect("cookie lock poisoned").as_ref() {
        Some(value) => println!("cookie: loaded ({})", mask_cookie(value)),
        None => {
            println!("cookie: not set");
            print_cookie_not_set_hint();
        }
    }

    println!();
    println!("Type /help for commands.");
    println!();

    initial_load(&chat_client, &cookie, &seen_ids, args.transcript_limit)?;

    let poll_handle = spawn_poller(
        chat_client.clone(),
        cookie.clone(),
        seen_ids.clone(),
        running.clone(),
        args.transcript_limit,
        args.poll_seconds,
    );

    let input_result = run_input_loop(&chat_client, &cookie, &cookie_file, running.clone());
    running.store(false, Ordering::SeqCst);

    if let Err(error) = poll_handle.join() {
        eprintln!("poller thread panicked: {error:?}");
    }

    input_result
}

fn initial_load(
    chat_client: &ChatClient,
    cookie: &Arc<RwLock<Option<String>>>,
    seen_ids: &Arc<Mutex<std::collections::HashSet<u64>>>,
    transcript_limit: usize,
) -> Result<()> {
    let cookie_value = cookie.read().expect("cookie lock poisoned").clone();
    match chat_client.fetch_recent_messages(transcript_limit, cookie_value.as_deref()) {
        Ok(messages) => {
            if messages.is_empty() {
                println!("No messages found.");
            } else {
                println!("Recent messages:");
                for message in &messages {
                    print_message("load", message);
                }
            }

            let mut seen = seen_ids.lock().expect("seen_ids lock poisoned");
            *seen = merge_seen_ids(&messages);
            println!();
        }
        Err(error) => {
            eprintln!("initial load failed: {error:#}");
            eprintln!("You can still set a cookie and continue.");
            println!();
        }
    }

    Ok(())
}

fn spawn_poller(
    chat_client: ChatClient,
    cookie: Arc<RwLock<Option<String>>>,
    seen_ids: Arc<Mutex<std::collections::HashSet<u64>>>,
    running: Arc<AtomicBool>,
    transcript_limit: usize,
    poll_seconds: u64,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while running.load(Ordering::SeqCst) {
            let cookie_value = cookie.read().expect("cookie lock poisoned").clone();
            match chat_client.fetch_recent_messages(transcript_limit, cookie_value.as_deref()) {
                Ok(messages) => {
                    let mut seen = seen_ids.lock().expect("seen_ids lock poisoned");
                    for message in messages {
                        if message.message_id == 0 || seen.insert(message.message_id) {
                            print_message("new", &message);
                        }
                    }
                }
                Err(error) => eprintln!("poll failed: {error:#}"),
            }

            thread::sleep(Duration::from_secs(poll_seconds.max(1)));
        }
    })
}

fn run_input_loop(
    chat_client: &ChatClient,
    cookie: &Arc<RwLock<Option<String>>>,
    cookie_file: &Path,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let stdin = io::stdin();

    loop {
        if cookie.read().expect("cookie lock poisoned").is_none() {
            print_cookie_not_set_hint();
        }

        print!("r15> ");
        io::stdout().flush().context("failed to flush stdout")?;

        let mut line = String::new();
        let read = stdin.read_line(&mut line).context("failed to read input")?;
        if read == 0 {
            println!();
            return Ok(());
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "/quit" || trimmed == "/exit" {
            println!("bye");
            return Ok(());
        }

        if trimmed == "/help" {
            print_help();
            continue;
        }

        if trimmed == "/show-cookie" {
            let current = cookie.read().expect("cookie lock poisoned");
            match current.as_ref() {
                Some(value) => println!("{}", mask_cookie(value)),
                None => println!("no cookie is set"),
            }
            continue;
        }

        if let Some(value) = trimmed.strip_prefix("/cookie ") {
            let existing = cookie.read().expect("cookie lock poisoned").clone();
            let cleaned = normalize_cookie_input(value, existing.as_deref());
            if cleaned.is_empty() {
                println!("cookie value is empty");
                continue;
            }

            save_cookie(cookie_file, &cleaned)?;
            *cookie.write().expect("cookie lock poisoned") = Some(cleaned.clone());
            println!("saved cookie {}", mask_cookie(&cleaned));
            continue;
        }

        if let Some(path) = trimmed.strip_prefix("/cookie-file ") {
            let cookie_path = PathBuf::from(path.trim());
            let loaded = fs::read_to_string(&cookie_path)
                .with_context(|| format!("failed to read cookie file {}", cookie_path.display()))?;
            let existing = cookie.read().expect("cookie lock poisoned").clone();
            let cleaned = normalize_cookie_input(&loaded, existing.as_deref());
            if cleaned.is_empty() {
                bail!("cookie file was empty");
            }

            save_cookie(cookie_file, &cleaned)?;
            *cookie.write().expect("cookie lock poisoned") = Some(cleaned.clone());
            println!("loaded cookie from {} ({})", cookie_path.display(), mask_cookie(&cleaned));
            continue;
        }

        if trimmed.starts_with('/') {
            println!("unknown command: {trimmed}");
            continue;
        }

        let cookie_value = cookie
            .read()
            .expect("cookie lock poisoned")
            .clone()
            .ok_or_else(|| anyhow::anyhow!("set a cookie first with /cookie or /cookie-file"))?;

        match chat_client.send_message(trimmed, &cookie_value) {
            Ok(_) => println!("[sent]"),
            Err(error) => eprintln!("send failed: {error:#}"),
        }

        if !running.load(Ordering::SeqCst) {
            return Ok(());
        }
    }
}

fn print_help() {
    println!("/help                show this help");
    println!("/cookie <value>      save or merge cookies from Cookie/Set-Cookie text");
    println!("/cookie-file <path>  load and merge cookies from a file");
    println!("/show-cookie         show a masked cookie preview");
    println!("/quit                exit the shell");
    println!("anything else        send a chat message");
}

fn print_cookie_not_set_hint() {
    println!("cookie not set");
    println!("set one with /cookie <cookie text> or /cookie-file <path>");
    println!();
}

fn print_message(kind: &str, message: &ChatMessage) {
    println!(
        "[{kind}] {} {}: {}",
        short_timestamp(&message.timestamp),
        message.user_name,
        message.text
    );
}

fn short_timestamp(timestamp: &str) -> &str {
    if timestamp.len() >= 19 {
        &timestamp[11..19]
    } else {
        timestamp
    }
}

fn load_cookie(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    let value = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let cleaned = normalize_cookie_input(&value, None);
    if cleaned.is_empty() {
        Ok(None)
    } else {
        Ok(Some(cleaned))
    }
}

fn save_cookie(path: &Path, cookie: &str) -> Result<()> {
    ensure_parent_dir(path)?;
    fs::write(path, format!("{cookie}\n")).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn normalize_cookie_input(value: &str, existing_cookie: Option<&str>) -> String {
    let mut merged_pairs = parse_cookie_pairs(existing_cookie.unwrap_or_default());
    let incoming_pairs = parse_cookie_pairs(value);

    if incoming_pairs.is_empty() {
        return String::new();
    }

    let mut index_by_name = HashMap::new();
    for (index, (name, _)) in merged_pairs.iter().enumerate() {
        index_by_name.insert(name.to_ascii_lowercase(), index);
    }

    for (name, cookie_value) in incoming_pairs {
        let lowered = name.to_ascii_lowercase();
        if let Some(index) = index_by_name.get(&lowered).copied() {
            merged_pairs[index].1 = cookie_value;
        } else {
            index_by_name.insert(lowered, merged_pairs.len());
            merged_pairs.push((name, cookie_value));
        }
    }

    merged_pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_cookie_pairs(value: &str) -> Vec<(String, String)> {
    let mut parsed = Vec::new();
    let mut seen_names = HashSet::new();

    for line in value.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let candidate = strip_cookie_prefix(trimmed);
        for segment in candidate.split(';') {
            let part = segment.trim();
            if part.is_empty() {
                continue;
            }

            let Some((raw_name, raw_value)) = part.split_once('=') else {
                continue;
            };

            let name = raw_name.trim();
            let lowered = name.to_ascii_lowercase();
            if is_cookie_attribute(&lowered) {
                continue;
            }

            let cookie_value = raw_value.trim();
            if cookie_value.is_empty() {
                continue;
            }

            if seen_names.insert(lowered) {
                parsed.push((name.to_string(), cookie_value.to_string()));
            }
        }
    }

    parsed
}

fn strip_cookie_prefix(value: &str) -> &str {
    let trimmed = value.trim();
    if let Some(rest) = trimmed
        .strip_prefix("Cookie:")
        .or_else(|| trimmed.strip_prefix("cookie:"))
        .or_else(|| trimmed.strip_prefix("Set-Cookie:"))
        .or_else(|| trimmed.strip_prefix("set-cookie:"))
    {
        rest.trim()
    } else {
        trimmed
    }
}

fn is_cookie_attribute(name: &str) -> bool {
    matches!(
        name,
        "expires"
            | "max-age"
            | "domain"
            | "path"
            | "secure"
            | "httponly"
            | "samesite"
            | "priority"
            | "partitioned"
    )
}

fn mask_cookie(cookie: &str) -> String {
    let trimmed = cookie.trim();
    let char_count = trimmed.chars().count();
    if char_count <= 16 {
        return "<set>".to_string();
    }

    let start: String = trimmed.chars().take(8).collect();
    let end: String = trimmed
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{start}...{end}")
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}
