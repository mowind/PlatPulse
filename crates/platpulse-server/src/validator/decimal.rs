//! Exact decimal arithmetic and block-rate helpers for Provider counters.

pub(super) fn validate_nonnegative_decimal(value: &str) -> Result<(), String> {
    let mut dots = 0;
    let mut digits = 0;
    for character in value.chars() {
        match character {
            '0'..='9' => digits += 1,
            '.' => dots += 1,
            _ => return Err("Provider returned an invalid numeric value".to_owned()),
        }
    }
    if digits == 0 || dots > 1 {
        return Err("Provider returned an invalid numeric value".to_owned());
    }
    Ok(())
}

/// One parsed bounded nonnegative decimal: a normalized integer part and the
/// source's exact fractional digits. Trailing fractional zeros are part of the
/// value and are preserved, because the aggregate must not invent or discard
/// source precision.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DecimalAmount {
    integer: String,
    fractional: String,
}

/// Parse nonnegative decimal syntax only: at least one digit, at most one dot.
/// It deliberately carries no length bound, because it is also used to re-read
/// an accumulated total, which can legitimately grow past any single source
/// value length by exactly the carry it needs to stay exact.
fn parse_decimal_digits(value: &str) -> Option<DecimalAmount> {
    let mut dots = 0;
    let mut digits = 0;
    for character in value.chars() {
        match character {
            '0'..='9' => digits += 1,
            '.' => dots += 1,
            _ => return None,
        }
    }
    if digits == 0 || dots > 1 {
        return None;
    }
    let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
    Some(DecimalAmount {
        integer: whole.trim_start_matches('0').to_owned(),
        fractional: fractional.to_owned(),
    })
}

/// Parse one bounded source decimal without touching binary floating point.
/// Accepts the same syntax as the trust boundary: at least one digit, at most
/// one dot, bounded length, and no control characters.
fn parse_decimal_amount(value: &str) -> Option<DecimalAmount> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return None;
    }
    parse_decimal_digits(value)
}

/// Render a parsed amount, treating an all-zero integer part as `0`.
fn render_decimal_amount(amount: &DecimalAmount) -> String {
    let integer = if amount.integer.is_empty() {
        "0"
    } else {
        amount.integer.as_str()
    };
    if amount.fractional.is_empty() {
        integer.to_owned()
    } else {
        format!("{integer}.{}", amount.fractional)
    }
}

/// Left-pad an amount's integer part and right-pad its fractional part so two
/// amounts can be added digit-by-digit at a shared scale.
fn scaled_decimal_digits(
    amount: &DecimalAmount,
    integer_width: usize,
    fraction_width: usize,
) -> String {
    let mut digits = String::with_capacity(integer_width + fraction_width);
    for _ in amount.integer.len()..integer_width {
        digits.push('0');
    }
    digits.push_str(&amount.integer);
    digits.push_str(&amount.fractional);
    for _ in amount.fractional.len()..fraction_width {
        digits.push('0');
    }
    digits
}

/// Add two equal-length digit strings and return the (possibly one digit
/// longer) base-10 sum.
fn add_decimal_digits(left: &str, right: &str) -> String {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut result = vec![0_u8; left.len() + 1];
    let mut carry = 0_u8;
    for index in (0..left.len()).rev() {
        let sum = (left[index] - b'0') + (right[index] - b'0') + carry;
        result[index + 1] = b'0' + (sum % 10);
        carry = sum / 10;
    }
    result[0] = b'0' + carry;
    String::from_utf8(result).expect("decimal digits are ASCII")
}

/// Fold one bounded nonnegative decimal string into an exact running total.
///
/// Provider amounts cross the trust boundary as source-precision decimal
/// strings and must never round-trip through binary floating point, so the
/// aggregate is plain string arithmetic. `total` starts as `None` (no known
/// value) and only becomes `Some` for an accepted value. The return value
/// reports whether `value` was accepted, so the caller can keep its coverage
/// and stale counts exact instead of guessing from the total.
pub fn accumulate_decimal(total: &mut Option<String>, value: &str) -> bool {
    let Some(incoming) = parse_decimal_amount(value) else {
        return false;
    };
    let Some(current) = total.as_deref() else {
        *total = Some(render_decimal_amount(&incoming));
        return true;
    };
    // The running total is always produced by render_decimal_amount, so it is
    // valid by construction; it is deliberately not re-subjected to the
    // per-source length bound, which would panic once the carry exceeds 256
    // digits.
    let current = parse_decimal_digits(current)
        .expect("an accumulated total is always a valid nonnegative decimal");
    let integer_width = current.integer.len().max(incoming.integer.len());
    let fraction_width = current.fractional.len().max(incoming.fractional.len());
    let left = scaled_decimal_digits(&current, integer_width, fraction_width);
    let right = scaled_decimal_digits(&incoming, integer_width, fraction_width);
    let sum = add_decimal_digits(&left, &right);
    let split = sum.len() - fraction_width;
    *total = Some(render_decimal_amount(&DecimalAmount {
        integer: sum[..split].trim_start_matches('0').to_owned(),
        fractional: sum[split..].to_owned(),
    }));
    true
}

/// Whether a validated nonnegative decimal string is greater than `max`.
/// The integer part is compared directly, so a percentage never round-trips
/// through a binary float.
pub(super) fn decimal_exceeds_max(value: &str, max: u64) -> bool {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u64>().unwrap_or(u64::MAX)
    };
    whole > max || (whole == max && fraction.bytes().any(|byte| byte != b'0'))
}

/// Cumulative actual / cumulative scheduled blocks from one successful
/// observation (#156). The returned percentage string carries no `%` sign and
/// is rounded half-up to six decimal places, mirroring the source's own rate
/// precision. Never combine a fresh numerator with an older denominator: both
/// inputs come from the same stored row.
///
/// * A `Some(0)` denominator is `not_applicable` — no scheduled duties —
///   regardless of whether the numerator is known, and is deliberately not
///   rendered as 0% or 100%.
/// * A missing half of the pair is `unknown`, never a synthesized zero.
/// * `ok` is the only state that carries a value.
pub fn cumulative_block_rate(
    block_count: Option<i64>,
    expected_block_count: Option<i64>,
) -> (Option<String>, &'static str) {
    match (block_count, expected_block_count) {
        (_, Some(0)) => (None, "not_applicable"),
        (Some(block_count), Some(expected_block_count)) => (
            Some(completion_rate_percentage(
                block_count,
                expected_block_count,
            )),
            "ok",
        ),
        _ => (None, "unknown"),
    }
}

/// Scale used by `completion_rate_percentage`: six fractional decimal places.
const RATE_SCALE: i128 = 1_000_000;

/// Render `numerator / denominator × 100` as a percentage string without the
/// `%` sign. Integer arithmetic only: the ratio never round-trips through a
/// binary float, and trailing zeros beyond the source's integer precision are
/// trimmed.
fn completion_rate_percentage(numerator: i64, denominator: i64) -> String {
    debug_assert!(denominator > 0);
    let numerator = i128::from(numerator.max(0));
    let denominator = i128::from(denominator);
    let scaled = (numerator * 100 * RATE_SCALE * 2 + denominator) / (denominator * 2);
    let integer = scaled / RATE_SCALE;
    let fraction = scaled % RATE_SCALE;
    if fraction == 0 {
        return integer.to_string();
    }
    let mut fraction = format!("{fraction:06}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{integer}.{fraction}")
}

pub(super) fn decimal_decreased(previous: Option<&str>, current: Option<&str>) -> bool {
    let Some((previous, current)) = previous.zip(current) else {
        return false;
    };
    let normalize = |value: &str| {
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        let whole = whole.trim_start_matches('0');
        let whole = if whole.is_empty() { "0" } else { whole };
        let fraction = fraction.trim_end_matches('0');
        (whole.to_owned(), fraction.to_owned())
    };
    let (previous_whole, previous_fraction) = normalize(previous);
    let (current_whole, current_fraction) = normalize(current);
    previous_whole.len() > current_whole.len()
        || (previous_whole.len() == current_whole.len()
            && (previous_whole > current_whole
                || (previous_whole == current_whole && {
                    let width = previous_fraction.len().max(current_fraction.len());
                    let previous_fraction = format!("{:0<width$}", previous_fraction);
                    let current_fraction = format!("{:0<width$}", current_fraction);
                    previous_fraction > current_fraction
                })))
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn decimal_accumulation_is_exact_and_rejects_malformed_values() {
        // Large integer and fractional magnitudes keep every source digit; a
        // binary float would already have lost them.
        let mut total: Option<String> = None;
        assert!(accumulate_decimal(
            &mut total,
            "111111111111111111111.111111111111"
        ));
        assert!(accumulate_decimal(
            &mut total,
            "222222222222222222222.222222222222"
        ));
        assert_eq!(total.as_deref(), Some("333333333333333333333.333333333333"));
        // Mixed precision pads to the widest source scale and preserves
        // trailing fractional zeros.
        let mut mixed: Option<String> = None;
        assert!(accumulate_decimal(&mut mixed, "10"));
        assert!(accumulate_decimal(&mut mixed, "0.10"));
        assert!(accumulate_decimal(&mut mixed, "2.2"));
        assert_eq!(mixed.as_deref(), Some("12.30"));
        // Leading-dot and trailing-dot syntax normalizes without inventing
        // precision beyond the source.
        let mut dotted: Option<String> = None;
        assert!(accumulate_decimal(&mut dotted, ".5"));
        assert!(accumulate_decimal(&mut dotted, ".5"));
        assert_eq!(dotted.as_deref(), Some("1.0"));
        let mut trailing_dot: Option<String> = None;
        assert!(accumulate_decimal(&mut trailing_dot, "12."));
        assert!(accumulate_decimal(&mut trailing_dot, "1"));
        assert_eq!(trailing_dot.as_deref(), Some("13"));
        // A valid zero stays a real value, distinct from "no value at all".
        let mut zero: Option<String> = None;
        assert!(accumulate_decimal(&mut zero, "0"));
        assert!(accumulate_decimal(&mut zero, "0.000"));
        assert_eq!(zero.as_deref(), Some("0.000"));
        // Malformed values are rejected without changing the running total.
        let mut rejected: Option<String> = None;
        assert!(accumulate_decimal(&mut rejected, "7"));
        for malformed in ["", "abc", "1e3", "-1", "1.2.3", "0x10", " 1"] {
            assert!(
                !accumulate_decimal(&mut rejected, malformed),
                "{malformed:?} must be rejected"
            );
        }
        assert_eq!(rejected.as_deref(), Some("7"));
        // An all-zero run normalizes the integer part instead of leaving
        // unbounded leading zeros.
        let mut leading: Option<String> = None;
        assert!(accumulate_decimal(&mut leading, "000.5"));
        assert!(accumulate_decimal(&mut leading, "000.5"));
        assert_eq!(leading.as_deref(), Some("1.0"));
    }

    #[test]
    fn decimal_accumulation_grows_past_the_source_bound_without_panicking() {
        // Each source value is within the 256-character trust-boundary bound,
        // but the exact running total legitimately grows one digit past it.
        // Re-reading that total must not re-apply the source bound and panic.
        let huge = "9".repeat(256);
        let mut total: Option<String> = None;
        for _ in 0..3 {
            assert!(accumulate_decimal(&mut total, &huge));
        }
        let expected = format!("2{}7", "9".repeat(255));
        assert_eq!(total.as_deref(), Some(expected.as_str()));
        assert_eq!(expected.len(), 257);
    }

    #[test]
    fn cumulative_block_rate_is_exact_and_never_mixes_observations() {
        assert_eq!(
            cumulative_block_rate(Some(100), Some(110)),
            (Some("90.909091".to_owned()), "ok")
        );
        assert_eq!(
            cumulative_block_rate(Some(1), Some(2)),
            (Some("50".to_owned()), "ok")
        );
        assert_eq!(
            cumulative_block_rate(Some(2), Some(3)),
            (Some("66.666667".to_owned()), "ok")
        );
        assert_eq!(
            cumulative_block_rate(Some(0), Some(5)),
            (Some("0".to_owned()), "ok")
        );
        // A known zero denominator is not applicable, never 0% or 100%,
        // whether or not the numerator is known.
        assert_eq!(
            cumulative_block_rate(Some(0), Some(0)),
            (None, "not_applicable")
        );
        assert_eq!(
            cumulative_block_rate(None, Some(0)),
            (None, "not_applicable")
        );
        // An incomplete pair stays unknown rather than mixing observations.
        assert_eq!(cumulative_block_rate(Some(200), None), (None, "unknown"));
        assert_eq!(cumulative_block_rate(None, Some(50)), (None, "unknown"));
        assert_eq!(cumulative_block_rate(None, None), (None, "unknown"));
    }
}
