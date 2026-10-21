//! The decimal digits of Java's `Double.toString`, which Jython's
//! `float.__repr__` is built on.
//!
//! Java (up to JDK 18, and Ignition ships Java 17) does not always produce
//! the shortest digit string that round-trips: large integral values get
//! their exact digits (`6.1987046510000005e+18` rather than `6.198704651e+18`)
//! and subnormals get two digits (`4.9e-324`). To print floats exactly like
//! Jython this module is a port of `jdk.internal.math.FloatingDecimal`'s
//! `BinaryToASCIIBuffer.dtoa`, including its overflow behaviour and its
//! asymmetric stopping tests.

const EXP_SHIFT: i32 = 52;
const FRACT_HOB: u64 = 1 << EXP_SHIFT;
const SIGNIF_BIT_MASK: u64 = FRACT_HOB - 1;
const EXP_BIAS: i32 = 1023;
const MAX_SMALL_BIN_EXP: i32 = 62;
const MIN_SMALL_BIN_EXP: i32 = -(63 / 3);

const N_5_BITS: [i32; 27] = [
    0, 3, 5, 7, 10, 12, 14, 17, 19, 21, 24, 26, 28, 31, 33, 35, 38, 40, 42, 45, 47, 49, 52, 54, 56,
    59, 61,
];

const INSIGNIFICANT_DIGITS_NUMBER: [i32; 64] = [
    0, 0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 5, 5, 5, 6, 6, 6, 6, 7, 7, 7, 8, 8, 8, 9, 9,
    9, 9, 10, 10, 10, 11, 11, 11, 12, 12, 12, 12, 13, 13, 13, 14, 14, 14, 15, 15, 15, 15, 16, 16,
    16, 17, 17, 17, 18, 18, 18, 19,
];

/// `5^i` for `i < 27` (fits in an `i64`).
fn long_5_pow(i: i32) -> i64 {
    5i64.pow(i as u32)
}

/// Digits and decimal exponent such that `value = 0.d1d2d3... * 10^exponent`.
/// `value` must be finite and positive.
pub(crate) fn digits(value: f64) -> (Vec<u8>, i32) {
    let bits = value.to_bits();
    let mut fract_bits = bits & SIGNIF_BIT_MASK;
    let mut bin_exp = ((bits >> EXP_SHIFT) & 0x7FF) as i32;
    let significant_bits;
    if bin_exp == 0 {
        // Subnormal (zero is handled by the caller).
        let leading_zeros = fract_bits.leading_zeros() as i32;
        let shift = leading_zeros - (63 - EXP_SHIFT);
        fract_bits <<= shift;
        bin_exp = 1 - shift;
        significant_bits = 64 - leading_zeros;
    } else {
        fract_bits |= FRACT_HOB;
        significant_bits = EXP_SHIFT + 1;
    }
    bin_exp -= EXP_BIAS;
    dtoa(bin_exp, fract_bits, significant_bits)
}

fn dtoa(bin_exp: i32, fract_bits: u64, n_significant_bits: i32) -> (Vec<u8>, i32) {
    let tail_zeros = fract_bits.trailing_zeros() as i32;
    let n_fract_bits = EXP_SHIFT + 1 - tail_zeros;
    let n_tiny_bits = (n_fract_bits - bin_exp - 1).max(0);

    if (MIN_SMALL_BIN_EXP..=MAX_SMALL_BIN_EXP).contains(&bin_exp)
        && n_tiny_bits < 27
        && n_fract_bits + N_5_BITS[n_tiny_bits as usize] < 64
        && n_tiny_bits == 0
    {
        let insignificant = if bin_exp > n_significant_bits {
            insignificant_digits_for_pow2(bin_exp - n_significant_bits - 1)
        } else {
            0
        };
        let value = if bin_exp >= EXP_SHIFT {
            fract_bits << (bin_exp - EXP_SHIFT)
        } else {
            fract_bits >> (EXP_SHIFT - bin_exp)
        };
        return develop_long_digits(0, value as i64, insignificant);
    }

    let mut dec_exp = estimate_dec_exp(fract_bits, bin_exp);
    let b5 = (-dec_exp).max(0);
    let mut b2 = b5 + n_tiny_bits + bin_exp;
    let s5 = dec_exp.max(0);
    let mut s2 = s5 + n_tiny_bits;
    let m5 = b5;
    let mut m2 = b2 - n_significant_bits;

    let fract = fract_bits >> tail_zeros;
    b2 -= n_fract_bits - 1;
    let common2 = b2.min(s2);
    b2 -= common2;
    s2 -= common2;
    m2 -= common2;
    if n_fract_bits == 1 {
        m2 -= 1;
    }
    if m2 < 0 {
        b2 -= m2;
        s2 -= m2;
        m2 = 0;
    }

    let b_bits = n_fract_bits
        + b2
        + if (b5 as usize) < N_5_BITS.len() {
            N_5_BITS[b5 as usize]
        } else {
            b5 * 3
        };
    let ten_s_bits = s2
        + 1
        + if ((s5 + 1) as usize) < N_5_BITS.len() {
            N_5_BITS[(s5 + 1) as usize]
        } else {
            (s5 + 1) * 3
        };

    let mut digits: Vec<u8> = Vec::with_capacity(20);
    let low;
    let high;
    let low_digit_difference: i64;

    if b_bits < 64 && ten_s_bits < 64 {
        if b_bits < 32 && ten_s_bits < 32 {
            // All values fit in an `int` (with Java's wrapping arithmetic).
            let mut b: i32 = (fract as i32)
                .wrapping_mul(long_5_pow(b5) as i32)
                .wrapping_shl(b2 as u32);
            let s: i32 = (long_5_pow(s5) as i32).wrapping_shl(s2 as u32);
            let mut m: i32 = (long_5_pow(m5) as i32).wrapping_shl(m2 as u32);
            let tens: i32 = s.wrapping_mul(10);
            let mut q = b / s;
            b = (b % s).wrapping_mul(10);
            m = m.wrapping_mul(10);
            let mut lo = b < m;
            let mut hi = b.wrapping_add(m) > tens;
            if q == 0 && !hi {
                dec_exp -= 1;
            } else {
                digits.push(q as u8);
            }
            if !(-3..8).contains(&dec_exp) {
                hi = false;
                lo = false;
            }
            while !lo && !hi {
                q = b / s;
                b = (b % s).wrapping_mul(10);
                m = m.wrapping_mul(10);
                if m > 0 {
                    lo = b < m;
                    hi = b.wrapping_add(m) > tens;
                } else {
                    lo = true;
                    hi = true;
                }
                digits.push(q as u8);
            }
            low_digit_difference = i64::from(b.wrapping_shl(1).wrapping_sub(tens));
            low = lo;
            high = hi;
        } else {
            // All values fit in a `long`.
            let mut b: i64 = (fract as i64)
                .wrapping_mul(long_5_pow(b5))
                .wrapping_shl(b2 as u32);
            let s: i64 = long_5_pow(s5).wrapping_shl(s2 as u32);
            let mut m: i64 = long_5_pow(m5).wrapping_shl(m2 as u32);
            let tens: i64 = s.wrapping_mul(10);
            let mut q = b / s;
            b = (b % s).wrapping_mul(10);
            m = m.wrapping_mul(10);
            let mut lo = b < m;
            let mut hi = b.wrapping_add(m) > tens;
            if q == 0 && !hi {
                dec_exp -= 1;
            } else {
                digits.push(q as u8);
            }
            if !(-3..8).contains(&dec_exp) {
                hi = false;
                lo = false;
            }
            while !lo && !hi {
                q = b / s;
                b = (b % s).wrapping_mul(10);
                m = m.wrapping_mul(10);
                if m > 0 {
                    lo = b < m;
                    hi = b.wrapping_add(m) > tens;
                } else {
                    lo = true;
                    hi = true;
                }
                digits.push(q as u8);
            }
            low_digit_difference = b.wrapping_shl(1).wrapping_sub(tens);
            low = lo;
            high = hi;
        }
    } else {
        // Arbitrary precision. The scaling Java applies to normalise the
        // divisor does not change any of the comparisons, so it is omitted.
        let s = BigUint::pow52(s5, s2);
        let mut b = BigUint::from_u64(fract).mul_pow52(b5, b2);
        let mut m = BigUint::pow52(m5 + 1, m2 + 1);
        let ten_s = BigUint::pow52(s5 + 1, s2 + 1);
        let mut q = b.quo_rem_iteration(&s);
        let mut lo = b < m;
        let mut hi = b.add(&m) >= ten_s;
        if q == 0 && !hi {
            dec_exp -= 1;
        } else {
            digits.push(q);
        }
        if !(-3..8).contains(&dec_exp) {
            hi = false;
            lo = false;
        }
        while !lo && !hi {
            q = b.quo_rem_iteration(&s);
            m = m.mul_small(10);
            lo = b < m;
            hi = b.add(&m) >= ten_s;
            digits.push(q);
        }
        low_digit_difference = if hi && lo {
            match b.mul_small(2).cmp(&ten_s) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        } else {
            0
        };
        low = lo;
        high = hi;
    }

    let mut dec_exponent = dec_exp + 1;
    if high {
        let round_up = if low {
            match low_digit_difference.cmp(&0) {
                std::cmp::Ordering::Equal => digits.last().is_some_and(|digit| digit & 1 != 0),
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
            }
        } else {
            true
        };
        if round_up {
            round_up_digits(&mut digits, &mut dec_exponent);
        }
    }
    (digits, dec_exponent)
}

fn round_up_digits(digits: &mut [u8], dec_exponent: &mut i32) {
    let Some(mut i) = digits.len().checked_sub(1) else {
        return;
    };
    let mut q = digits[i];
    if q == 9 {
        while q == 9 && i > 0 {
            digits[i] = 0;
            i -= 1;
            q = digits[i];
        }
        if q == 9 {
            *dec_exponent += 1;
            digits[0] = 1;
            return;
        }
    }
    digits[i] = q + 1;
}

fn develop_long_digits(
    mut dec_exponent: i32,
    mut lvalue: i64,
    insignificant: i32,
) -> (Vec<u8>, i32) {
    if insignificant != 0 {
        let pow10 = long_5_pow(insignificant) << insignificant;
        let residue = lvalue % pow10;
        lvalue /= pow10;
        dec_exponent += insignificant;
        if residue >= (pow10 >> 1) {
            lvalue += 1;
        }
    }
    let mut reversed = Vec::with_capacity(20);
    let mut c = lvalue % 10;
    lvalue /= 10;
    while c == 0 {
        dec_exponent += 1;
        c = lvalue % 10;
        lvalue /= 10;
    }
    while lvalue != 0 {
        reversed.push(c as u8);
        dec_exponent += 1;
        c = lvalue % 10;
        lvalue /= 10;
    }
    reversed.push(c as u8);
    reversed.reverse();
    (reversed, dec_exponent + 1)
}

fn insignificant_digits_for_pow2(p2: i32) -> i32 {
    if p2 > 1 && (p2 as usize) < INSIGNIFICANT_DIGITS_NUMBER.len() {
        INSIGNIFICANT_DIGITS_NUMBER[p2 as usize]
    } else {
        0
    }
}

/// `FloatingDecimal.estimateDecExp`, with the same floating-point arithmetic.
fn estimate_dec_exp(fract_bits: u64, bin_exp: i32) -> i32 {
    const EXP_ONE: u64 = 0x3FF0_0000_0000_0000;
    let d2 = f64::from_bits(EXP_ONE | (fract_bits & SIGNIF_BIT_MASK));
    // Java's constant, deliberately not `LOG10_2` (it differs in the last
    // digit and must round the same way).
    #[allow(clippy::approx_constant)]
    let d = (d2 - 1.5) * 0.289529654 + 0.176091259 + f64::from(bin_exp) * 0.301029995663981;
    let d_bits = d.to_bits();
    let exponent = ((d_bits >> EXP_SHIFT) & 0x7FF) as i32 - EXP_BIAS;
    let negative = d_bits >> 63 != 0;
    if (0..52).contains(&exponent) {
        let mask = SIGNIF_BIT_MASK >> exponent;
        let r = (((d_bits & SIGNIF_BIT_MASK) | FRACT_HOB) >> (EXP_SHIFT - exponent)) as i32;
        if negative {
            if mask & d_bits == 0 { -r } else { -r - 1 }
        } else {
            r
        }
    } else if exponent < 0 {
        if d_bits & !(1 << 63) == 0 {
            0
        } else if negative {
            -1
        } else {
            0
        }
    } else {
        d as i32
    }
}

/// A minimal unsigned big integer (little-endian base 2^32 limbs).
#[derive(Clone, Debug, PartialEq, Eq)]
struct BigUint(Vec<u32>);

impl BigUint {
    fn from_u64(value: u64) -> Self {
        let mut limbs = vec![value as u32, (value >> 32) as u32];
        trim(&mut limbs);
        BigUint(limbs)
    }

    /// `5^p5 * 2^p2`.
    fn pow52(p5: i32, p2: i32) -> Self {
        BigUint::from_u64(1).mul_pow52(p5, p2)
    }

    /// `self * 5^p5 * 2^p2`.
    fn mul_pow52(mut self, p5: i32, p2: i32) -> Self {
        for _ in 0..p5 {
            self = self.mul_small(5);
        }
        self.shl(p2.max(0) as u32)
    }

    fn mul_small(&self, factor: u32) -> Self {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut carry: u64 = 0;
        for &limb in &self.0 {
            let value = u64::from(limb) * u64::from(factor) + carry;
            out.push(value as u32);
            carry = value >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        trim(&mut out);
        BigUint(out)
    }

    fn shl(&self, bits: u32) -> Self {
        let words = (bits / 32) as usize;
        let bits = bits % 32;
        let mut out = vec![0u32; words];
        let mut carry: u32 = 0;
        for &limb in &self.0 {
            if bits == 0 {
                out.push(limb);
            } else {
                out.push((limb << bits) | carry);
                carry = limb >> (32 - bits);
            }
        }
        if carry > 0 {
            out.push(carry);
        }
        trim(&mut out);
        BigUint(out)
    }

    fn add(&self, other: &BigUint) -> BigUint {
        let len = self.0.len().max(other.0.len());
        let mut out = Vec::with_capacity(len + 1);
        let mut carry: u64 = 0;
        for i in 0..len {
            let value = u64::from(*self.0.get(i).unwrap_or(&0))
                + u64::from(*other.0.get(i).unwrap_or(&0))
                + carry;
            out.push(value as u32);
            carry = value >> 32;
        }
        if carry > 0 {
            out.push(carry as u32);
        }
        trim(&mut out);
        BigUint(out)
    }

    fn sub_assign(&mut self, other: &BigUint) {
        let mut borrow: i64 = 0;
        for i in 0..self.0.len() {
            let value = i64::from(self.0[i]) - i64::from(*other.0.get(i).unwrap_or(&0)) - borrow;
            if value < 0 {
                self.0[i] = (value + (1 << 32)) as u32;
                borrow = 1;
            } else {
                self.0[i] = value as u32;
                borrow = 0;
            }
        }
        trim(&mut self.0);
    }

    /// `q = self / divisor` (which must be below 10); `self = 10 * (self % divisor)`.
    fn quo_rem_iteration(&mut self, divisor: &BigUint) -> u8 {
        let mut q = 0;
        while *self >= *divisor && q < 10 {
            self.sub_assign(divisor);
            q += 1;
        }
        *self = self.mul_small(10);
        q
    }
}

impl PartialOrd for BigUint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigUint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

fn trim(limbs: &mut Vec<u32>) {
    while limbs.last() == Some(&0) {
        limbs.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn java(value: f64) -> String {
        let (digits, exponent) = digits(value);
        let digits: String = digits
            .iter()
            .map(|digit| char::from(b'0' + digit))
            .collect();
        format!("0.{digits}e{exponent}")
    }

    #[test]
    fn matches_java_double_to_string() {
        assert_eq!(java(1.0), "0.1e1");
        assert_eq!(java(0.1), "0.1e0");
        assert_eq!(java(5e-324), "0.49e-323");
        assert_eq!(java(6198704651e9), "0.61987046510000005e19");
        assert_eq!(java(1e16), "0.1e17");
        assert_eq!(java(123.456), "0.123456e3");
    }
}
