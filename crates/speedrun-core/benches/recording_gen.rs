/// Deterministic asciicast v2 recording generator for benchmarks.
///
/// Generates a complete asciicast v2 recording with realistic-looking
/// terminal output including ANSI escape sequences.
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;

/// Generate a complete asciicast v2 recording as bytes.
///
/// - `duration_secs`: approximate duration of the recording in seconds
/// - `seed`: RNG seed for determinism
///
/// Returns a valid asciicast v2 recording with ~3 output events per second,
/// mixed printable text and ANSI escape sequences.
pub fn generate_recording(duration_secs: u64, seed: u64) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut output = Vec::new();

    // Write the asciicast v2 header
    let header = r#"{"version":2,"width":80,"height":24}"#;
    output.extend_from_slice(header.as_bytes());
    output.push(b'\n');

    let mut clock: f64 = 0.0;
    let duration = duration_secs as f64;

    while clock < duration {
        // Advance time: between 0.2 and 0.7 seconds per event (~1.4 to 5 events/sec, averaging ~3)
        let gap: f64 = 0.2 + rng.random::<f64>() * 0.5;
        clock += gap;

        if clock > duration {
            break;
        }

        // Generate event data: randomly pick between different output types
        let data = match rng.random_range(0u32..3) {
            0 => generate_text_line(&mut rng),
            1 => generate_sgr_sequence(&mut rng),
            _ => generate_cursor_movement(&mut rng),
        };

        // Escape for JSON string
        let escaped = json_escape(&data);

        // Write the event: [timestamp, "o", "data"]
        let line = format!("[{clock:.6},\"o\",\"{escaped}\"]\n");
        output.extend_from_slice(line.as_bytes());
    }

    output
}

/// Generate a short text line with newline (e.g., shell prompt output).
fn generate_text_line(rng: &mut StdRng) -> String {
    let commands = [
        "$ ls -la\r\n",
        "$ git status\r\n",
        "$ cargo build\r\n",
        "total 42\r\n",
        "drwxr-xr-x  5 user group  160 Jan  1 12:00 .\r\n",
        "-rw-r--r--  1 user group 1234 Jan  1 12:00 Cargo.toml\r\n",
        "$ cd src\r\n",
        "On branch main\r\n",
        "nothing to commit\r\n",
        "   Compiling speedrun v0.1.0\r\n",
    ];
    let idx = rng.random_range(0..commands.len());
    commands[idx].to_string()
}

/// Generate an SGR color sequence with some text.
fn generate_sgr_sequence(rng: &mut StdRng) -> String {
    let color_n = rng.random_range(0u32..256);
    let texts = ["error", "warning", "info", "ok", "done", "failed", "pass"];
    let text = texts[rng.random_range(0..texts.len())];
    format!("\x1b[38;5;{color_n}m{text}\x1b[0m\r\n")
}

/// Generate a cursor movement sequence with some text.
fn generate_cursor_movement(rng: &mut StdRng) -> String {
    let row = rng.random_range(1u32..24);
    let col = rng.random_range(1u32..80);
    let texts = ["#", ">", "*", "-", "+"];
    let text = texts[rng.random_range(0..texts.len())];
    format!("\x1b[{row};{col}H{text}")
}

/// Escape a string for use in a JSON string value.
/// Handles backslash, double-quote, and common control characters.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Escape other control chars as \uXXXX
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}
