# Scalar math standard library

Import `/vendor/stasis/stdlib/math.stasis` for the deterministic scalar math
helpers. Decimal literals normally resolve as `f32`; use an explicitly typed
`f64` local to select an `f64` overload and keep the wider result:

```stasis
let precise_value: f64 = 2.0;
let precise_exponent: f64 = 0.5;
let precise_root: f64 = math_sqrt(precise_value);
let precise_power: f64 = math_pow(precise_value, precise_exponent);
```

The math routines are implemented in shared Stasis source, so target parity
checks use the same edge behavior and accuracy limits for JIT, native AOT,
Wasm, Android, and iOS consumers.

| Function | Result and units | Edge behavior |
| --- | --- | --- |
| `math_sqrt(f32)` / `math_sqrt(f64)` | Square root in the input's scale | Negative values and either signed zero return positive zero. Positive infinity is preserved; negative infinity returns positive zero; NaN propagates. The `f32` overload uses the correctly rounded f32 square-root primitive. |
| `math_pow(base, exponent)` | Power in the base's result lane | The base selects `f32` or `f64` result precision; the exponent may be `i32`, `f32`, or `f64`. Any zero exponent returns one, including for zero or NaN bases. A finite negative nonzero base with a finite non-integral floating exponent returns NaN. Integer powers preserve odd/even sign behavior, including signed zero; negative integer powers invert the factor before repeated squaring, including the minimum i32 exponent. |
| `math_exp(f32/f64)` | Natural exponential; dimensionless | NaN propagates; positive infinity stays positive infinity and negative infinity becomes positive zero. Finite overflow and underflow produce the corresponding infinity, subnormal, or zero result. |
| `math_ln(f32/f64)` | Natural logarithm; dimensionless | Positive finite values, including subnormals, are accepted. Zero returns negative infinity; negative values return NaN; positive infinity is preserved; NaN propagates. |
| `math_log10(f32/f64)` | Base-10 logarithm; dimensionless | Has the same domain and exceptional-value behavior as `math_ln`. |
| `math_floor(f32/f64)` | Greatest integral value no greater than the input | Rounds toward negative infinity. NaN, infinities, and signed zero are returned unchanged. |
| `math_ceil(f32/f64)` | Least integral value no less than the input | Rounds toward positive infinity. NaN, infinities, and signed zero are returned unchanged. |
| `math_round(f32/f64)` | Nearest integral value | Halfway cases round toward positive infinity (`-1.5` becomes `-1`, `2.5` becomes `3`). NaN, infinities, and signed zero are returned unchanged; negative inputs that round to zero keep a negative-zero result. |
| `math_min(f32/f64)` / `math_max(f32/f64)` | The smaller or larger input in its lane | Any NaN input propagates. For equal signed zeros, minimum selects negative zero and maximum selects positive zero, independent of argument order. |
| `math_clamp(f32/f64)` | Input bounded to the supplied interval | Reversed bounds are swapped. An input already inside the interval is returned as-is, preserving its zero sign. Any NaN input propagates. |
| `math_atan2_degrees(y, x)` | Angle in degrees, range `[-180, 180]`; positive angles turn clockwise in screen coordinates where positive Y points down | `(0, 0)` returns zero. Both signs of zero are treated as zero; the negative X axis returns `180`. Inputs are finite f32 coordinates. |
| `math_acos_degrees(x)` | Inverse cosine in degrees, range `[0, 180]` | Inputs below `-1` clamp to `180`; inputs above `1` clamp to zero. |
| `math_abs(f32/f64)` | Absolute magnitude in the input lane | Both signed zeros return positive zero. Infinities become positive infinity; NaN propagates. |
| `math_abs(i32)` | Absolute i32 magnitude | `i32::MIN` saturates to `i32::MAX`, the largest representable positive result. |

Power special values follow this implementation's branch order: a zero exponent
returns one for any base, and base one returns one even for a NaN exponent;
other NaN operands produce NaN. For an infinite floating exponent,
`abs(base) == 1` returns one. When `abs(base) > 1`, positive infinity as the
exponent returns positive infinity and negative infinity returns positive
zero; when `abs(base) < 1`, those results reverse. With a finite exponent and
an infinite base, positive exponents produce infinity and negative exponents
produce zero. Odd integral exponents preserve the negative sign for a negative
infinite base, including negative zero for negative exponents. A negative
infinity base with a finite non-integral exponent returns NaN; a positive
infinity base with a finite non-integral exponent follows the exponent-sign
infinity/zero rule.

Integer-exponent powers use a fixed 32-step repeated-square loop. A floating
exponent that is integral and within `[-1000000, 1000000]` uses that same path;
other finite floating powers use `exp(exponent * ln(abs(base)))`, with the
negative-base sign restored for odd integral exponents. Mixed `f32`/`f64`
`math_pow` calls still return the base lane: an `f64` base with an `f32`
exponent returns `f64`, while an `f32` base with an `f64` exponent returns
`f32`.

The current accuracy oracle uses these limits:

- `f32` square root is correctly rounded. The inverse-angle functions use a
  range-reduced alternating-series approximation and shared f32 operations on
  every target; their contract is finite f32 inputs and absolute error at most
  `0.01` degree.
- Normal finite `f32` powers, logarithms, and exponential checks use relative
  error at most `3e-6`. A subnormal exponential result is checked with an
  absolute allowance of one minimum positive f32 subnormal and its
  zero/subnormal classification is checked separately. For subnormal powers,
  the absolute allowance is the larger of `3e-6 * abs(expected)` and the
  minimum positive f32 subnormal.
- Positive finite `f64` square roots use relative error at most `1e-12`.
  `f64` exponential checks over `[-700, 700]` use relative error at most
  `1e-9`; subnormal results use an absolute allowance of one minimum positive
  f64 subnormal, with zero/subnormal classification checked separately.
- Positive finite `f64` logarithms, including subnormals, use absolute error at
  most `1e-12 * max(1, abs(expected))`. Near one, `math_ln` is checked to
  absolute error at most `2e-16` so adjacent input values remain observable.
- General `f64` powers with `abs(exponent) <= 1000000` use relative error at
  most `1e-9` for normal results. For subnormal results the absolute allowance
  is the larger of `1e-9 * abs(expected)` and the minimum positive f64
  subnormal. Power accuracy is not specified as a strict one-ULP bound; for
  example, `10^-309` may differ by several ULPs while remaining within this
  relative tolerance.

## Short f64 ASCII formatting

`ascii_write_f64_short(dst, value, decimals, base)` writes at the supplied
destination offset and returns the new logical length, or `-1` on invalid
arguments or insufficient capacity. `decimals` must be `0..6`. The
`ascii_from_f64_short(dst, value, decimals)` wrapper starts at offset zero;
`ascii_append_f64_short(dst, value, decimals)` appends at the current logical
length.

The formatter selects notation from the absolute input magnitude: nonzero
values below `0.01` or at least `1000000` use scientific notation; values from
`0.01` up to but not including `1000` use the requested fixed decimal count;
values from `1000` to below `1000000` are rounded to an integer. The selected
band is based on the input before rounding, so a fixed result can carry to
`1000.00`, while a scientific significand can carry its exponent, as in
`9.999e300` becoming `1.00e301` at two decimals. Scientific output uses a
lowercase `e`, an optional minus sign, and no padded exponent digits.

Rounding is performed on the magnitude with a guard digit and carry, then the
sign is emitted; exact halfway cases round away from zero. NaN and infinities
are lowercase `nan`, `inf`, and `-inf`. Signed zero is emitted as unsigned
zero, with the requested decimals (for example, `-0.0` at two decimals is
`0.00`). Digits are staged as local integers, then written directly to the
destination after capacity preflight; the formatter does not call a host
formatter or allocate. Preflight reserves the trailing NUL; invalid precision,
negative offset, or insufficient capacity returns `-1` without changing the
destination bytes or recorded length.

The compiler's pure `f32_to_bits(f32): i32` primitive preserves the raw IEEE-754
binary32 representation. It is used by the shared cross-target math oracle to
compare exact outputs, including signed zero.
