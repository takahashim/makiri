//! Lexbor's decimal-to-double conversion: `core/strtod.c` and the DiyFp
//! arithmetic of `core/diyfp.{h,c}` it rests on.
//!
//! It is not correctly rounded (there is no big-number fallback), so it is
//! ported rather than replaced by `str::parse`: a CSS number's value reaches
//! the selector AST through `An+B`, and the port has to give Lexbor's value.

/// `lexbor_diyfp_t`.
#[derive(Clone, Copy)]
struct DiyFp {
    significand: u64,
    exp: i32,
}

const DBL_SIGNIFICAND_SIZE: i32 = 52;
const DBL_EXPONENT_BIAS: i32 = 0x3FF + DBL_SIGNIFICAND_SIZE;
const DBL_EXPONENT_MAX: i32 = 0x7FF - DBL_EXPONENT_BIAS;
const DBL_EXPONENT_DENORMAL: i32 = -DBL_EXPONENT_BIAS + 1;
const DBL_SIGNIFICAND_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;
const DBL_HIDDEN_BIT: u64 = 0x0010_0000_0000_0000;

const DIYFP_SIGNIFICAND_SIZE: i32 = 64;
const SIGNIFICAND_SIZE: i32 = 53;

const DECIMAL_EXPONENT_OFF: i32 = 348;
const DECIMAL_EXPONENT_MIN: i32 = -348;
const DECIMAL_EXPONENT_DIST: i32 = 8;

const DECIMAL_POWER_MAX: i32 = 309;
const DECIMAL_POWER_MIN: i32 = -324;
const UINT64_DECIMAL_DIGITS_MAX: i32 = 19;
const DENOM_LOG: i32 = 3;
const DENOM: i64 = 1 << DENOM_LOG;

/// `lexbor_cached_powers`: (significand, binary exponent, decimal exponent).
const CACHED_POWERS: [(u64, i16, i16); 87] = [
    (0xfa8fd5a0_081c0288, -1220, -348),
    (0xbaaee17f_a23ebf76, -1193, -340),
    (0x8b16fb20_3055ac76, -1166, -332),
    (0xcf42894a_5dce35ea, -1140, -324),
    (0x9a6bb0aa_55653b2d, -1113, -316),
    (0xe61acf03_3d1a45df, -1087, -308),
    (0xab70fe17_c79ac6ca, -1060, -300),
    (0xff77b1fc_bebcdc4f, -1034, -292),
    (0xbe5691ef_416bd60c, -1007, -284),
    (0x8dd01fad_907ffc3c, -980, -276),
    (0xd3515c28_31559a83, -954, -268),
    (0x9d71ac8f_ada6c9b5, -927, -260),
    (0xea9c2277_23ee8bcb, -901, -252),
    (0xaecc4991_4078536d, -874, -244),
    (0x823c1279_5db6ce57, -847, -236),
    (0xc2109436_4dfb5637, -821, -228),
    (0x9096ea6f_3848984f, -794, -220),
    (0xd77485cb_25823ac7, -768, -212),
    (0xa086cfcd_97bf97f4, -741, -204),
    (0xef340a98_172aace5, -715, -196),
    (0xb23867fb_2a35b28e, -688, -188),
    (0x84c8d4df_d2c63f3b, -661, -180),
    (0xc5dd4427_1ad3cdba, -635, -172),
    (0x936b9fce_bb25c996, -608, -164),
    (0xdbac6c24_7d62a584, -582, -156),
    (0xa3ab6658_0d5fdaf6, -555, -148),
    (0xf3e2f893_dec3f126, -529, -140),
    (0xb5b5ada8_aaff80b8, -502, -132),
    (0x87625f05_6c7c4a8b, -475, -124),
    (0xc9bcff60_34c13053, -449, -116),
    (0x964e858c_91ba2655, -422, -108),
    (0xdff97724_70297ebd, -396, -100),
    (0xa6dfbd9f_b8e5b88f, -369, -92),
    (0xf8a95fcf_88747d94, -343, -84),
    (0xb9447093_8fa89bcf, -316, -76),
    (0x8a08f0f8_bf0f156b, -289, -68),
    (0xcdb02555_653131b6, -263, -60),
    (0x993fe2c6_d07b7fac, -236, -52),
    (0xe45c10c4_2a2b3b06, -210, -44),
    (0xaa242499_697392d3, -183, -36),
    (0xfd87b5f2_8300ca0e, -157, -28),
    (0xbce50864_92111aeb, -130, -20),
    (0x8cbccc09_6f5088cc, -103, -12),
    (0xd1b71758_e219652c, -77, -4),
    (0x9c400000_00000000, -50, 4),
    (0xe8d4a510_00000000, -24, 12),
    (0xad78ebc5_ac620000, 3, 20),
    (0x813f3978_f8940984, 30, 28),
    (0xc097ce7b_c90715b3, 56, 36),
    (0x8f7e32ce_7bea5c70, 83, 44),
    (0xd5d238a4_abe98068, 109, 52),
    (0x9f4f2726_179a2245, 136, 60),
    (0xed63a231_d4c4fb27, 162, 68),
    (0xb0de6538_8cc8ada8, 189, 76),
    (0x83c7088e_1aab65db, 216, 84),
    (0xc45d1df9_42711d9a, 242, 92),
    (0x924d692c_a61be758, 269, 100),
    (0xda01ee64_1a708dea, 295, 108),
    (0xa26da399_9aef774a, 322, 116),
    (0xf209787b_b47d6b85, 348, 124),
    (0xb454e4a1_79dd1877, 375, 132),
    (0x865b8692_5b9bc5c2, 402, 140),
    (0xc83553c5_c8965d3d, 428, 148),
    (0x952ab45c_fa97a0b3, 455, 156),
    (0xde469fbd_99a05fe3, 481, 164),
    (0xa59bc234_db398c25, 508, 172),
    (0xf6c69a72_a3989f5c, 534, 180),
    (0xb7dcbf53_54e9bece, 561, 188),
    (0x88fcf317_f22241e2, 588, 196),
    (0xcc20ce9b_d35c78a5, 614, 204),
    (0x98165af3_7b2153df, 641, 212),
    (0xe2a0b5dc_971f303a, 667, 220),
    (0xa8d9d153_5ce3b396, 694, 228),
    (0xfb9b7cd9_a4a7443c, 720, 236),
    (0xbb764c4c_a7a44410, 747, 244),
    (0x8bab8eef_b6409c1a, 774, 252),
    (0xd01fef10_a657842c, 800, 260),
    (0x9b10a4e5_e9913129, 827, 268),
    (0xe7109bfb_a19c0c9d, 853, 276),
    (0xac2820d9_623bf429, 880, 284),
    (0x80444b5e_7aa7cf85, 907, 292),
    (0xbf21e440_03acdd2d, 933, 300),
    (0x8e679c2f_5e44ff8f, 960, 308),
    (0xd433179d_9c8cb841, 986, 316),
    (0x9e19db92_b4e31ba9, 1013, 324),
    (0xeb96bf6e_badf77d9, 1039, 332),
    (0xaf87023b_9bf0ee6b, 1066, 340),
];

impl DiyFp {
    #[inline]
    fn new(significand: u64, exp: i32) -> DiyFp {
        DiyFp { significand, exp }
    }

    /// `lexbor_diyfp_shift_left`.
    fn shift_left(self, shift: u32) -> DiyFp {
        DiyFp::new(self.significand << shift, self.exp - shift as i32)
    }

    /// `lexbor_diyfp_shift_right`.
    fn shift_right(self, shift: u32) -> DiyFp {
        DiyFp::new(self.significand >> shift, self.exp + shift as i32)
    }

    /// `lexbor_diyfp_normalize`. Never called on a zero significand.
    fn normalize(self) -> DiyFp {
        self.shift_left(self.significand.leading_zeros())
    }

    /// `lexbor_diyfp_mul`, the 128-bit form (the 32-bit form rounds the same).
    fn mul(self, rhs: DiyFp) -> DiyFp {
        let p = u128::from(self.significand) * u128::from(rhs.significand);
        let mut h = (p >> 64) as u64;
        let l = p as u64;
        if l & (1 << 63) != 0 {
            h = h.wrapping_add(1);
        }
        DiyFp::new(h, self.exp + rhs.exp + 64)
    }

    /// `lexbor_diyfp_2d`.
    fn to_f64(self) -> f64 {
        let mut exp = self.exp;
        let mut significand = self.significand;
        while significand > DBL_HIDDEN_BIT + DBL_SIGNIFICAND_MASK {
            significand >>= 1;
            exp += 1;
        }
        if exp >= DBL_EXPONENT_MAX {
            return f64::INFINITY;
        }
        if exp < DBL_EXPONENT_DENORMAL {
            return 0.0;
        }
        while exp > DBL_EXPONENT_DENORMAL && significand & DBL_HIDDEN_BIT == 0 {
            significand <<= 1;
            exp -= 1;
        }
        let biased_exp = if exp == DBL_EXPONENT_DENORMAL && significand & DBL_HIDDEN_BIT == 0 {
            0
        } else {
            (exp + DBL_EXPONENT_BIAS) as u64
        };
        f64::from_bits((significand & DBL_SIGNIFICAND_MASK) | (biased_exp << DBL_SIGNIFICAND_SIZE))
    }
}

/// `lexbor_cached_power_dec`. `exp` is at least `DECIMAL_EXPONENT_MIN` and
/// below `DECIMAL_POWER_MAX`, so the index is in the table.
fn cached_power_dec(exp: i32) -> (DiyFp, i32) {
    let index = ((exp + DECIMAL_EXPONENT_OFF) / DECIMAL_EXPONENT_DIST) as usize;
    let (significand, bin_exp, dec_exp) = CACHED_POWERS
        .get(index)
        .copied()
        .unwrap_or(CACHED_POWERS[CACHED_POWERS.len() - 1]);
    (
        DiyFp::new(significand, i32::from(bin_exp)),
        i32::from(dec_exp),
    )
}

/// `lexbor_strtod_read_uint64`.
fn read_uint64(digits: &[u8]) -> (u64, usize) {
    let mut value: u64 = 0;
    let mut read = 0;
    while read < digits.len() && value < u64::MAX / 10 {
        value = 10 * value + u64::from(digits[read].wrapping_sub(b'0'));
        read += 1;
    }
    (value, read)
}

/// `lexbor_strtod_diyfp_read`.
fn diyfp_read(digits: &[u8]) -> (DiyFp, i32) {
    let (mut significand, read) = read_uint64(digits);
    if digits.len() != read && digits[read] >= b'5' {
        significand += 1;
    }
    (DiyFp::new(significand, 0), (digits.len() - read) as i32)
}

/// `lexbor_strtod_adjust_pow10`.
fn adjust_pow10(exp: i32) -> DiyFp {
    match exp {
        1 => DiyFp::new(0xa000_0000_0000_0000, -60),
        2 => DiyFp::new(0xc800_0000_0000_0000, -57),
        3 => DiyFp::new(0xfa00_0000_0000_0000, -54),
        4 => DiyFp::new(0x9c40_0000_0000_0000, -50),
        5 => DiyFp::new(0xc350_0000_0000_0000, -47),
        6 => DiyFp::new(0xf424_0000_0000_0000, -44),
        7 => DiyFp::new(0x9896_8000_0000_0000, -40),
        _ => DiyFp::new(0, 0),
    }
}

/// `lexbor_strtod_diyfp_sgnd_size`.
fn sgnd_size(order: i32) -> i32 {
    if order >= DBL_EXPONENT_DENORMAL + SIGNIFICAND_SIZE {
        return SIGNIFICAND_SIZE;
    }
    if order <= DBL_EXPONENT_DENORMAL {
        return 0;
    }
    order - DBL_EXPONENT_DENORMAL
}

/// `lexbor_strtod_diyfp_strtod`.
fn diyfp_strtod(digits: &[u8], mut exp: i32) -> f64 {
    let (mut value, remaining) = diyfp_read(digits);
    exp += remaining;

    let mut error: i64 = if remaining == 0 { 0 } else { DENOM / 2 };
    let orig_e = value.exp;
    value = value.normalize();
    error <<= orig_e - value.exp;

    if exp < DECIMAL_EXPONENT_MIN {
        return 0.0;
    }

    let (pow, dec_exp) = cached_power_dec(exp);
    if dec_exp != exp {
        let adj_exp = exp - dec_exp;
        value = value.mul(adjust_pow10(adj_exp));
        if UINT64_DECIMAL_DIGITS_MAX - (digits.len() as i32) < adj_exp {
            error += DENOM / 2;
        }
    }

    value = value.mul(pow);
    error += DENOM + i64::from(error != 0);

    let orig_e = value.exp;
    value = value.normalize();
    error <<= orig_e - value.exp;

    let magnitude = DIYFP_SIGNIFICAND_SIZE + value.exp;
    let mut prec_digits = DIYFP_SIGNIFICAND_SIZE - sgnd_size(magnitude);

    if prec_digits + DENOM_LOG >= DIYFP_SIGNIFICAND_SIZE {
        let shift = prec_digits + DENOM_LOG - DIYFP_SIGNIFICAND_SIZE + 1;
        value = value.shift_right(shift as u32);
        error = (error >> shift) + 1 + DENOM;
        prec_digits -= shift;
    }

    let prec_bits = (value.significand & ((1u64 << prec_digits) - 1)).wrapping_mul(DENOM as u64);
    let half_way = (1u64 << (prec_digits - 1)).wrapping_mul(DENOM as u64);

    let mut rounded = value.shift_right(prec_digits as u32);
    if prec_bits >= half_way.wrapping_add(error as u64) {
        rounded.significand += 1;
    }
    rounded.to_f64()
}

/// `lexbor_strtod_internal`: the value of the decimal digits `digits` times
/// ten to `exp`.
pub fn strtod_internal(digits: &[u8], mut exp: i32) -> f64 {
    /* Trim leading zeroes. */
    let lead = digits.iter().position(|&d| d != b'0');
    let Some(lead) = lead else {
        return 0.0;
    };
    let start = &digits[lead..];
    let left = start.len();

    /* Trim trailing zeroes. */
    let mut p = left - 1;
    while p > 0 && start[p] == b'0' {
        p -= 1;
    }
    let right = p + 1;
    let start = &start[..right];

    exp += (left - right) as i32;

    if exp + right as i32 > DECIMAL_POWER_MAX {
        return f64::INFINITY;
    }
    if exp + right as i32 <= DECIMAL_POWER_MIN {
        return 0.0;
    }
    diyfp_strtod(start, exp)
}

#[cfg(test)]
mod tests {
    use super::strtod_internal;

    #[test]
    fn integers_read_exactly() {
        assert_eq!(strtod_internal(b"0", 0), 0.0);
        assert_eq!(strtod_internal(b"000", 0), 0.0);
        assert_eq!(strtod_internal(b"1", 0), 1.0);
        assert_eq!(strtod_internal(b"42", 0), 42.0);
        assert_eq!(strtod_internal(b"1000", 0), 1000.0);
        assert_eq!(strtod_internal(b"9007199254740993", 0), 9007199254740992.0);
        assert_eq!(strtod_internal(b"15", -1), 1.5);
        assert_eq!(strtod_internal(b"1", 400), f64::INFINITY);
    }
}
