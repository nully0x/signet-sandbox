// Human-friendly duration parsing for CLI flags and config fields.

/// Parse a duration like `90s`, `20m`, `1h`, `2d`, `1h30m`, or bare seconds.
pub fn parse_duration_secs(input: &str) -> anyhow::Result<u64> {
    anyhow::ensure!(!input.is_empty(), "empty duration");
    let mut total: u64 = 0;
    let mut digits = String::new();
    for ch in input.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        anyhow::ensure!(!digits.is_empty(), "invalid duration `{input}`");
        let multiple = unit_secs(ch)
            .ok_or_else(|| anyhow::anyhow!("invalid duration `{input}`: unknown unit `{ch}`"))?;
        total = add(total, &digits, multiple, input)?;
        digits.clear();
    }
    if !digits.is_empty() {
        total = add(total, &digits, 1, input)?;
    }
    anyhow::ensure!(total > 0, "duration must be positive");
    Ok(total)
}

fn add(total: u64, digits: &str, multiple: u64, input: &str) -> anyhow::Result<u64> {
    let n: u64 = digits
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid duration `{input}`"))?;
    total
        .checked_add(
            n.checked_mul(multiple).ok_or_else(|| {
                anyhow::anyhow!("invalid duration `{input}`: value overflows u64")
            })?,
        )
        .ok_or_else(|| anyhow::anyhow!("invalid duration `{input}`: value overflows u64"))
}

fn unit_secs(ch: char) -> Option<u64> {
    match ch {
        's' => Some(1),
        'm' => Some(60),
        'h' => Some(3600),
        'd' => Some(86400),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_units() {
        assert_eq!(parse_duration_secs("90s").unwrap(), 90);
        assert_eq!(parse_duration_secs("20m").unwrap(), 1200);
        assert_eq!(parse_duration_secs("1h").unwrap(), 3600);
        assert_eq!(parse_duration_secs("2d").unwrap(), 172_800);
    }

    #[test]
    fn parses_bare_seconds_and_compound() {
        assert_eq!(parse_duration_secs("30").unwrap(), 30);
        assert_eq!(parse_duration_secs("1h30m").unwrap(), 5400);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_duration_secs("").is_err());
        assert!(parse_duration_secs("m").is_err());
        assert!(parse_duration_secs("0m").is_err());
        assert!(parse_duration_secs("1x").is_err());
        assert!(parse_duration_secs("-5m").is_err());
        assert!(parse_duration_secs("99999999999999999999d").is_err());
    }
}
