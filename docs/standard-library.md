# Stasis Standard Library Design

This document defines the standard library for the Stasis programming language.

## Design Principles

1. **Prefix by module**: `str_`, `io_`, `sys_`, `math_`, `ascii_`, `mem_`, `gfx_`
2. **UTF-8 everywhere**: All strings use the `string[N]` type (UTF-8 payload + length headers)
3. **In-place operations**: Modify destination buffer, return length or success
4. **Consistent naming**: `module_verb_noun` pattern
5. **No app-specific functions**: Generic primitives only
6. **Explicit about bytes vs codepoints**: Function names and docs clearly indicate which unit they operate on

---

## String Model

Stasis strings use a dedicated `string[N]` type with UTF-8 payload and tracked lengths.

- **Type**: `string[N]` where `N` is the maximum payload size in bytes (not counting headers)
- **Layout (in memory)**: `[byte_length: i32][char_length: i32][data: u8[N]]`
- **UTF-8 encoded payload**: `data` holds the UTF-8 bytes; `byte_length` is the used byte count
- **Codepoint-aware**: `char_length` tracks the decoded codepoint count for the current payload
- **Byte-indexed by default**: APIs accept byte indices for performance, but maintain codepoint counts
- **Null sentinel**: `data[byte_length]` is always set to `0` for interop; the sentinel is not counted in `N`
- **Header-driven ops**: `str_*` functions read and write the headers; raw byte helpers require a recount if they change payload bytes

### UTF-8 Encoding Basics

| Codepoint Range | Bytes | First Byte Pattern | Continuation Bytes |
|-----------------|-------|--------------------|--------------------|
| U+0000 - U+007F | 1 | `0xxxxxxx` | none |
| U+0080 - U+07FF | 2 | `110xxxxx` | 1 (`10xxxxxx`) |
| U+0800 - U+FFFF | 3 | `1110xxxx` | 2 |
| U+10000 - U+10FFFF | 4 | `11110xxx` | 3 |

### Key Implications

1. **ASCII is a subset**: Bytes 0-127 are identical to ASCII and always single-byte
2. **Byte length differs from character count**: "h\xe2\x82\xacllo" is 7 bytes but 6 characters
3. **Random byte access can split characters**: Indexing into the middle of a multi-byte sequence produces invalid UTF-8
4. **Searching for ASCII is safe**: ASCII bytes (0-127) never appear as continuation bytes
5. **Headers must stay in sync**: Any change to payload bytes must update `byte_length` and `char_length`

### Safety Guidelines

**Safe operations on UTF-8 strings:**
- Searching for ASCII characters (`,`, `\n`, `/`, etc.)
- Comparing strings byte-by-byte (UTF-8 maintains sort order)
- Copying/appending entire strings
- Measuring byte length
- Iterating from start to end, respecting codepoint boundaries

**Unsafe without care:**
- Truncating at arbitrary byte positions (may split a codepoint)
- Extracting substrings by byte index (may start/end mid-codepoint)
- Case conversion (only reliable for ASCII a-z/A-Z)

---

## Module: `ascii_` (ASCII Byte Utilities)

Classification and conversion for **ASCII bytes only** (0-127). These functions operate on single bytes and are only meaningful for ASCII characters. For UTF-8 strings, use these only on bytes you know to be ASCII.

### Classification

```stasis
ascii_is_digit(b: u8): bool      // '0'-'9' (48-57)
ascii_is_alpha(b: u8): bool      // 'a'-'z', 'A'-'Z'
ascii_is_alnum(b: u8): bool      // digit or alpha
ascii_is_space(b: u8): bool      // ' ', '\t', '\n', '\r' (32, 9, 10, 13)
ascii_is_upper(b: u8): bool      // 'A'-'Z' (65-90)
ascii_is_lower(b: u8): bool      // 'a'-'z' (97-122)
ascii_is_hex(b: u8): bool        // '0'-'9', 'a'-'f', 'A'-'F'
ascii_is_print(b: u8): bool      // printable ASCII (32-126)
ascii_is_ascii(b: u8): bool      // true if b < 128
```

### Conversion

```stasis
ascii_to_upper(b: u8): u8        // 'a'->'A', others unchanged
ascii_to_lower(b: u8): u8        // 'A'->'a', others unchanged
ascii_to_digit(b: u8): i32       // '0'->0, '9'->9, else -1
ascii_from_digit(d: i32): u8     // 0->'0', 9->'9', else '?'
ascii_to_hex(b: u8): i32         // '0'->0, 'a'->10, 'F'->15, else -1
ascii_from_hex(d: i32): u8       // 0->'0', 10->'a', else '?'
```

---

## Module: `str_` (String Operations)

All string functions operate on `string[N]` values (UTF-8 payload + length headers). Unless noted, mutating functions update both `byte_length` and `char_length` and maintain the null sentinel.

### Length

```stasis
str_len(s: string[]): i32              // Byte length from header (fast)
str_len_utf8(s: string[]): i32         // Codepoint count from header
str_is_empty(s: string[]): bool        // Returns byte_length == 0
str_recount_utf8(s: string[]): i32     // Recompute lengths from payload, fix headers
```

### Byte Access (Low-Level)

These operate on raw bytes and do not adjust headers. Call `str_recount_utf8` after manual edits to resync lengths.

```stasis
str_get_byte(s: string[], index: i32): u8       // Get byte at index (0 if out of bounds)
str_set_byte(s: string[], index: i32, b: u8)    // Set byte at index (no-op if out of bounds)
```

### UTF-8 Iteration

```stasis
str_next_codepoint(s: string[], byte_index: i32): i32
    // Returns byte index of next codepoint start, or -1 if at end
    // Usage: iterate codepoints without decoding

str_decode_codepoint(s: string[], byte_index: i32): i32
    // Decode codepoint at byte_index, return Unicode codepoint value
    // Returns -1 if invalid UTF-8 or out of bounds

str_encode_codepoint(dst: string[], codepoint: i32): i32
    // Encode codepoint into dst payload, update lengths, return bytes written (1-4)
    // Returns 0 if invalid codepoint or no capacity
```

### Comparison

All comparisons are byte-wise, which preserves UTF-8 lexicographic order.

```stasis
str_eq(a: string[], b: string[]): bool                 // Byte-wise equality
str_cmp(a: string[], b: string[]): i32                 // -1, 0, 1 (lexicographic)
str_starts_with(s: string[], prefix: string[]): bool   // Check prefix match
str_ends_with(s: string[], suffix: string[]): bool     // Check suffix match
```

### Search (Returns Byte Index)

These return **byte indices**. When searching for ASCII characters, results are always safe to use for splitting.

```stasis
str_find(s: string[], needle: string[]): i32           // First byte index of needle, or -1
str_find_byte(s: string[], b: u8): i32                 // First byte index of byte, or -1
str_find_last_byte(s: string[], b: u8): i32            // Last byte index of byte, or -1
str_contains(s: string[], needle: string[]): bool      // True if s contains needle
```

### Modification (In-Place)

```stasis
str_clear(s: string[])                                // Reset lengths to 0 and write null sentinel
str_copy(dst: string[], src: string[]): i32           // Copy src to dst, return bytes copied
str_append(dst: string[], src: string[]): i32         // Append src to dst, return new byte length
str_append_byte(dst: string[], b: u8): i32            // Append single byte (must be ASCII), return new length
str_append_codepoint(dst: string[], cp: i32): i32     // Append UTF-8 encoded codepoint, return bytes written
```

### Substring (Byte-Based)

These use byte indices. Caller must ensure indices don't split UTF-8 codepoints.

```stasis
str_substr(dst: string[], src: string[], start: i32, byte_len: i32): i32
    // Extract byte_len bytes starting at byte index start
    // Returns bytes copied
```

### Trimming (In-Place, ASCII Whitespace)

Trims ASCII whitespace (space, tab, newline, carriage return). Safe for UTF-8 since these are all single-byte. Length headers are updated.

```stasis
str_trim_start(s: string[]): i32    // Remove leading ASCII whitespace
str_trim_end(s: string[]): i32      // Remove trailing ASCII whitespace
str_trim(s: string[]): i32          // Remove both, return new byte length
```

### Case Conversion (ASCII Only, In-Place)

Only converts ASCII letters (a-z, A-Z). Non-ASCII UTF-8 bytes are unchanged. Length headers are preserved.

```stasis
str_to_upper(s: string[])           // Convert ASCII lowercase to uppercase
str_to_lower(s: string[])           // Convert ASCII uppercase to lowercase
```

### Number Conversion

```stasis
str_from_i32(dst: string[], value: i32): i32                   // Int to string, return byte length
str_from_f32(dst: string[], value: f32, decimals: i32): i32    // Float to string
str_to_i32(s: string[]): i32                                   // Parse int (0 on failure)
str_to_f32(s: string[]): f32                                   // Parse float (0.0 on failure)
```

### Validation

```stasis
str_is_valid_utf8(s: string[]): bool              // Check if string is valid UTF-8
str_sanitize_utf8(s: string[]): i32               // Replace invalid sequences with U+FFFD and fix headers
```

---

## Module: `io_` (Input/Output)

### Console Output

```stasis
io_print(s: string[])             // Print UTF-8 string (no newline)
io_println(s: string[])           // Print UTF-8 string + newline
io_print_i32(value: i32)          // Print integer
io_print_f32(value: f32)          // Print float
io_print_codepoint(cp: i32)       // Print single Unicode codepoint (UTF-8 encoded)
io_print_bool(value: bool)        // Print "true" or "false"
io_newline()                      // Print newline
```

### Console Input

```stasis
io_read_byte(): i32               // Read single byte (blocking), -1 on EOF
io_read_line(dst: string[]): i32  // Read UTF-8 line into buffer, return byte length
io_read_i32(): i32                // Read and parse integer
```

---

## Module: `sys_` (System)

### Time

```stasis
sys_time_sec(): i32             // Unix timestamp in seconds
sys_time_ms(): i32              // Milliseconds since program start
sys_sleep_ms(ms: i32)           // Sleep for milliseconds
```

### Platform Info

```stasis
sys_platform(): i32             // 0=unknown, 1=windows, 2=linux, 3=macos
sys_arch(): i32                 // 0=unknown, 1=x86_64, 2=arm64, 3=wasm32
sys_pointer_size(): i32         // 4 or 8 bytes
```

### Program Control

```stasis
sys_exit(code: i32)             // Exit program with code
sys_abort()                     // Abort immediately
```

### Random Numbers

```stasis
sys_random_seed(seed: i32)              // Seed the PRNG
sys_random(): i32                       // Next random i32
sys_random_range(min: i32, max: i32): i32  // Random in [min, max)
```

---

## Module: `math_` (Mathematics)

### Trigonometry

```stasis
math_sin(x: f32): f32           // Precise sine
math_cos(x: f32): f32           // Precise cosine
math_tan(x: f32): f32           // Tangent
math_sin_fast(x: f32): f32      // Fast sine (less precise)
math_cos_fast(x: f32): f32      // Fast cosine (less precise)
math_atan2(y: f32, x: f32): f32 // Arc tangent of y/x
```

### Exponential / Logarithm

```stasis
math_sqrt(x: f32): f32          // Square root
math_pow(base: f32, exp: f32): f32  // Power
math_exp(x: f32): f32           // e^x
math_log(x: f32): f32           // Natural log
math_log10(x: f32): f32         // Base-10 log
```

### Rounding

```stasis
math_floor(x: f32): f32         // Round down
math_ceil(x: f32): f32          // Round up
math_round(x: f32): f32         // Round to nearest
math_trunc(x: f32): f32         // Truncate toward zero
```

### Absolute Value / Min / Max

```stasis
math_abs_i32(x: i32): i32
math_abs_f32(x: f32): f32
math_min_i32(a: i32, b: i32): i32
math_max_i32(a: i32, b: i32): i32
math_min_f32(a: f32, b: f32): f32
math_max_f32(a: f32, b: f32): f32
math_clamp_i32(x: i32, min: i32, max: i32): i32
math_clamp_f32(x: f32, min: f32, max: f32): f32
```

### Sign

```stasis
math_sign_i32(x: i32): i32      // -1, 0, or 1
math_sign_f32(x: f32): f32      // -1.0, 0.0, or 1.0
```

### Constants

```stasis
const MATH_PI: f32 = 3.14159265;
const MATH_TAU: f32 = 6.28318530;
const MATH_E: f32 = 2.71828182;
```

---

## Module: `mem_` (Memory Operations)

Raw byte operations on buffers.

```stasis
mem_copy(dst: u8[], src: u8[], count: i32)     // Copy count bytes
mem_set(dst: u8[], value: u8, count: i32)      // Fill with value
mem_zero(dst: u8[], count: i32)                // Fill with zeros
mem_cmp(a: u8[], b: u8[], count: i32): i32     // Compare bytes, return -1/0/1
```

---

## Module: `gfx_` (Graphics - External Runtime)

These are implemented in `runtime/stasis_graphics.c`, not as compiler built-ins.

```stasis
gfx_init(width: i32, height: i32, title: string[]): bool
gfx_begin_frame()
gfx_end_frame()
gfx_clear(r: f32, g: f32, b: f32, a: f32)
gfx_draw_line(x1: f32, y1: f32, x2: f32, y2: f32, r: f32, g: f32, b: f32, a: f32)
gfx_is_key_down(scancode: i32): bool
gfx_should_quit(): bool
```

---

## Implementation Priority

### Phase 1: Essential (Self-Hosting Foundation)

- [ ] `ascii_*` module (all functions)
- [ ] `str_*` module (core: len, copy, append, compare, find)
- [ ] `sys_exit`, `sys_abort`

### Phase 2: UTF-8 Support

- [ ] `str_len_utf8` - codepoint counting and header sync
- [ ] `str_next_codepoint`, `str_decode_codepoint`, `str_encode_codepoint`
- [ ] `str_is_valid_utf8`, `str_sanitize_utf8`
- [ ] `str_append_codepoint`

### Phase 3: Utilities

- [ ] `math_sqrt`, `math_abs_*`, `math_min_*`, `math_max_*`, `math_clamp_*`
- [ ] `io_println`, `io_read_line`, `io_newline`
- [ ] `mem_*` functions
- [ ] `sys_random*`
- [ ] Number conversion functions

### Phase 4: Extended Math

- [ ] `math_atan2`, `math_pow`, `math_floor`, `math_ceil`, `math_round`
- [ ] `math_tan`, `math_exp`, `math_log`, `math_log10`

### Phase 5: Cleanup

- [ ] Remove legacy Sudoku functions
- [ ] Rename legacy functions to new naming convention
- [ ] Deprecation warnings for old names

---

## Current Built-in Functions (Legacy)

### To Remove (App-Specific)

| Function           | Reason                    |
| ------------------ | ------------------------- |
| `print_cell`       | Sudoku-specific rendering |
| `print_prompt`     | Sudoku-specific prompt    |
| `print_invalid`    | Sudoku-specific error     |
| `print_clue_error` | Sudoku-specific error     |
| `print_solved`     | Sudoku-specific message   |

### To Rename (Consistency)

| Old Name       | New Name       |
| -------------- | -------------- |
| `print_string` | `io_print`     |
| `print_int`    | `io_print_i32` |
| `print_char`   | `io_print_codepoint` |
| `read_char`    | `io_read_byte` |
| `read_int`     | `io_read_i32`  |
| `time`         | `sys_time_sec` |
| `get_time_ms`  | `sys_time_ms`  |
| `sleep_ms`     | `sys_sleep_ms` |
| `sin`          | `math_sin`     |
| `cos`          | `math_cos`     |
| `sin_fast`     | `math_sin_fast`|
| `cos_fast`     | `math_cos_fast`|

---

## Migration Guide

For existing code using legacy function names:

```stasis
// Old way:
print_string("Hello");
print_int(42);
let c: i32 = read_char();
let t: i32 = time();

// New way:
io_print("Hello");
io_print_i32(42);
let c: i32 = io_read_byte();
let t: i32 = sys_time_sec();
```

---

## UTF-8 Usage Examples

### Iterating Over Codepoints

```stasis
global text: string[256];
global i: i32;
global cp: i32;

function print_codepoints() {
    i = 0;
    while (i >= 0) {
        cp = str_decode_codepoint(text, i);
        if (cp >= 0) {
            io_print_codepoint(cp);
            io_print(" ");
        }
        i = str_next_codepoint(text, i);
    }
}
```

### Safe String Splitting on ASCII Delimiter

```stasis
global path: string[256];
global filename: string[64];

function extract_filename() {
    // Find last '/' - safe because '/' is ASCII
    let slash_pos: i32 = str_find_last_byte(path, 47);  // 47 = '/'
    if (slash_pos >= 0) {
        str_substr(filename, path, slash_pos + 1, str_len(path) - slash_pos - 1);
    } else {
        str_copy(filename, path);
    }
}
```
