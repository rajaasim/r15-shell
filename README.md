# r15-shell

`r15-shell` is a small Rust command-line client for Stack Overflow chat.

It is built for a simple workflow:

- load recent room messages in the terminal
- poll for new messages
- send messages from the shell
- save and reuse a session cookie

## Features

- standalone CLI
- room transcript polling
- message posting through the normal chat endpoint
- local cookie persistence
- simple in-shell commands for cookie management
- GitHub Actions builds for Windows, Linux, and macOS

## Project Status

Early and intentionally small. The current version focuses on chat basics rather than a full-screen terminal UI.

## Build

```bash
cargo build --release
```

## Run

```bash
cargo run -- --room-id 15
```

Use a specific cookie file if needed:

```bash
cargo run -- --room-id 15 --cookie-file ./cookie_header.txt
```

## Shell Commands

- `/help`
- `/cookie <cookie text>`
- `/cookie-file <path>`
- `/show-cookie`
- `/quit`

Any input that does not start with `/` is sent as a chat message.

## Cookie Setup

`r15-shell` accepts either a normal `Cookie` header or browser-style `Set-Cookie` text from a working `chat.stackoverflow.com` session.

Example:

```text
cf_clearance=...; chatusr=...; __cf_bm=...
```

This also works:

```text
chatusr=t=...&p=[53|True]; expires=Sun, 27 Sep 2026 13:42:34 GMT; path=/; secure; httponly
```

The shell now:

- ignores `expires`, `path`, `secure`, `httponly`, and similar cookie attributes
- accepts `Cookie:` and `Set-Cookie:` prefixes
- merges pasted cookies into the existing saved cookie header instead of replacing everything blindly

## Releases

GitHub Actions builds release binaries for:

- Windows
- Linux
- macOS

Artifacts are available from the Actions tab for each workflow run.

Pushing a tag like `v0.1.0` also creates a GitHub Release and attaches ZIP downloads for all three platforms.

## Notes

- Message loading currently uses the room transcript page rather than the websocket feed.
- Some authenticated requests may still be blocked by Stack Overflow or Cloudflare if the session cookie is stale or challenged.
- Background polling can print while you are typing until a richer terminal renderer is added.
