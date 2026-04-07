use crate::qb::TorrentInfo;

/// Format byte count into human-readable string.
pub fn fmt_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut val = bytes as f64;
    let mut idx = 0;
    while val >= 1024.0 && idx < units.len() - 1 {
        val /= 1024.0;
        idx += 1;
    }
    format!("{:.1} {}", val, units[idx])
}

/// Format bytes-per-second as speed string.
pub fn fmt_speed(bps: u64) -> String {
    if bps == 0 {
        return "0 B/s".to_string();
    }
    format!("{}/s", fmt_size(bps))
}

/// Format ETA seconds into H:MM:SS or "∞".
pub fn fmt_eta(seconds: i64) -> String {
    if seconds <= 0 || seconds >= 8_640_000 {
        return "∞".to_string();
    }
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// Return the status emoji for a torrent state string.
pub fn state_emoji(state: &str) -> &'static str {
    match state {
        "downloading" | "forcedDL" => "⏬",
        "uploading" | "forcedUP" => "⏫",
        "pausedDL" | "pausedUP" => "⏸️",
        "stoppedDL" | "stoppedUP" => "⏹️",
        "queuedDL" | "queuedUP" => "⏯️",
        "checkingDL" | "checkingUP" => "🔍",
        "error" => "❗",
        "missingFiles" => "⚠️",
        "stalledDL" | "stalledUP" => "⚙️",
        "metaDL" => "📡",
        "allocating" => "💾",
        "moving" => "📦",
        _ => "❓",
    }
}

/// Format a single torrent with full details (4 lines).
pub fn format_detail(t: &TorrentInfo) -> String {
    format!(
        "{} {}\n  Progress: {:.1}%  |  {} / {}\n  ↓ {}  ↑ {}  Ratio: {:.2}\n  Peers: {}/{}  |  ETA: {}",
        state_emoji(&t.state),
        t.name,
        t.progress * 100.0,
        fmt_size(t.completed),
        fmt_size(t.size),
        fmt_speed(t.dlspeed),
        fmt_speed(t.upspeed),
        t.ratio,
        t.num_leechs,
        t.num_seeds,
        fmt_eta(t.eta),
    )
}

/// Format a single torrent as a brief one-liner.
pub fn format_brief(t: &TorrentInfo) -> String {
    format!(
        "{} {} — {:.1}% of {}",
        state_emoji(&t.state),
        t.name,
        t.progress * 100.0,
        fmt_size(t.size),
    )
}

/// Format a list of torrents sorted by activity then progress.
pub fn format_list(torrents: &[TorrentInfo], limit: usize, detailed: bool) -> String {
    if torrents.is_empty() {
        return "No torrents found.".to_string();
    }

    fn state_order(state: &str) -> i64 {
        match state {
            "downloading" | "forcedDL" | "metaDL" => 0,
            "uploading" | "forcedUP" => 2,
            _ => 3,
        }
    }

    let mut sorted: Vec<&TorrentInfo> = torrents.iter().collect();
    sorted.sort_by(|a, b| {
        let oa = state_order(&a.state);
        let ob = state_order(&b.state);
        oa.cmp(&ob)
            .then_with(|| b.progress.partial_cmp(&a.progress).unwrap_or(std::cmp::Ordering::Equal))
    });

    let shown = sorted.iter().take(limit);
    let mut entries: Vec<String> = shown
        .map(|t| if detailed { format_detail(t) } else { format_brief(t) })
        .collect();

    if sorted.len() > limit {
        entries.push(format!("…and {} more (use a filter command)", sorted.len() - limit));
    }

    entries.join("\n\n")
}

/// Split text into chunks that fit within Telegram's 4096-char message limit.
/// Splits on double-newlines first (torrent entry boundaries), then single newlines,
/// then hard-splits at character boundaries to avoid breaking UTF-8 sequences.
pub fn chunk_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for block in text.split("\n\n") {
        let candidate = if current.is_empty() {
            block.to_string()
        } else {
            format!("{}\n\n{}", current, block)
        };

        if candidate.len() <= max_len {
            current = candidate;
        } else {
            if !current.is_empty() {
                chunks.push(current.clone());
                current = String::new();
            }
            if block.len() > max_len {
                // Block itself is too large — split on single newlines
                let mut line_chunk = String::new();
                for line in block.split('\n') {
                    let candidate = if line_chunk.is_empty() {
                        line.to_string()
                    } else {
                        format!("{}\n{}", line_chunk, line)
                    };
                    if candidate.len() <= max_len {
                        line_chunk = candidate;
                    } else {
                        if !line_chunk.is_empty() {
                            chunks.push(line_chunk.clone());
                            line_chunk = String::new();
                        }
                        if line.len() > max_len {
                            // Hard split at character boundaries
                            let mut start = 0;
                            while start < line.len() {
                                let mut end = (start + max_len).min(line.len());
                                while !line.is_char_boundary(end) {
                                    end -= 1;
                                }
                                chunks.push(line[start..end].to_string());
                                start = end;
                            }
                        } else {
                            line_chunk = line.to_string();
                        }
                    }
                }
                if !line_chunk.is_empty() {
                    current = line_chunk;
                }
            } else {
                current = block.to_string();
            }
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_size_units() {
        assert_eq!(fmt_size(0), "0.0 B");
        assert_eq!(fmt_size(1023), "1023.0 B");
        assert_eq!(fmt_size(1024), "1.0 KB");
        assert_eq!(fmt_size(1024 * 1024), "1.0 MB");
        assert_eq!(fmt_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(fmt_size(1024u64.pow(4)), "1.0 TB");
    }

    #[test]
    fn fmt_eta_values() {
        assert_eq!(fmt_eta(-1), "∞");
        assert_eq!(fmt_eta(0), "∞");
        assert_eq!(fmt_eta(8_640_000), "∞");
        assert_eq!(fmt_eta(60), "1:00");
        assert_eq!(fmt_eta(90), "1:30");
        assert_eq!(fmt_eta(3661), "1:01:01");
    }

    #[test]
    fn chunk_text_short() {
        let text = "hello";
        assert_eq!(chunk_text(text, 4096), vec!["hello"]);
    }

    #[test]
    fn chunk_text_splits_on_double_newline() {
        let block_a = "a".repeat(100);
        let block_b = "b".repeat(100);
        let text = format!("{}\n\n{}", block_a, block_b);
        // Should fit in one chunk
        let chunks = chunk_text(&text, 4096);
        assert_eq!(chunks.len(), 1);

        // Force a split: max_len = 150 (each block 100 chars + separator)
        let chunks = chunk_text(&text, 150);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], block_a);
        assert_eq!(chunks[1], block_b);
    }

    #[test]
    fn chunk_text_handles_emoji() {
        // "🎉" is 4 bytes — ensure no mid-sequence split
        let text = "🎉".repeat(2000); // 8000 bytes
        let chunks = chunk_text(&text, 4096);
        for chunk in &chunks {
            // Must be valid UTF-8 (will panic if not)
            let _ = chunk.as_str();
        }
    }

    #[test]
    fn format_list_empty() {
        assert_eq!(format_list(&[], 10, true), "No torrents found.");
    }
}
