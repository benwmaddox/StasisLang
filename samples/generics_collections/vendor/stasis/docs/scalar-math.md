# Scalar math standard library

Import `/vendor/stasis/stdlib/math.stasis` for the deterministic scalar math
helpers.

| Function | Result and units | Edge behavior |
| --- | --- | --- |
| `math_sqrt(f32)` | Square root in the input's scale | Negative and zero inputs return positive zero. Positive finite inputs use the correctly rounded host f32 square-root primitive. Positive infinity is preserved; negative infinity returns positive zero; NaN propagates. |
| `math_atan2_degrees(y, x)` | Angle in degrees, range `[-180, 180]`; positive angles turn clockwise in screen coordinates where positive Y points down | `(0, 0)` returns zero. Both signs of zero are treated as zero; the negative X axis returns `180`. Inputs are finite f32 coordinates. |
| `math_acos_degrees(x)` | Inverse cosine in degrees, range `[0, 180]` | Inputs below `-1` clamp to `180`; inputs above `1` clamp to zero. |
| `math_abs(f32)` | Absolute f32 magnitude | Both signed zeros return positive zero. Infinities become positive infinity; NaN propagates. |
| `math_abs(i32)` | Absolute i32 magnitude | `i32::MIN` saturates to `i32::MAX`, the largest representable positive result. |

The inverse-angle functions use a range-reduced alternating-series approximation
and shared f32 operations on every target. Their input contract is finite f32
values; NaN and infinity are outside it. Their test contract is absolute error
at most `0.01` degree. `math_sqrt` is tested to relative error at most `1e-6`
from tiny through large finite magnitudes.

The compiler's pure `f32_to_bits(f32): i32` primitive preserves the raw IEEE-754
binary32 representation. It is used by the shared cross-target math oracle to
compare exact outputs, including signed zero.
