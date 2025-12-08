# Diagnostic Style (Elm-Inspired)

Goal: concise, actionable diagnostics that help users fix code quickly, inspired by Elm’s clarity.

Principles
- Lead with the problem in plain language; avoid jargon. Example: “Only one assignment is allowed per expression.”
- Always point to the location and the construct name (e.g., “in assignment to `x`”).
- Include a short hint that suggests a fix, not just the rule.
- Prefer positive framing where possible (“Use infix `=` for assignment”) and avoid blaming phrasing.
- Be consistent in terminology: “assignment”, “operator-method”, “struct reference”, “global array element”.
- Keep severity implied (errors vs warnings) without shouting; no all-caps.

Formatting
- Single-line primary message; follow-up hint separated by “Hint:” or a second sentence.
- If showing examples, use minimal code (one or two lines) and keep ASCII.
- Group multiple related diagnostics when possible (e.g., aggregate multiple missing semicolons).

Common Cases (apply the pattern above)
- Multiple assignments in one expression: “Only one assignment is allowed per expression. Hint: split into two statements.”
- Legacy `.=` usage: “Use infix `=` for assignment. Hint: replace `a.=(b)` with `a = b`.”
- Non-assignable target: “Assignments must target an identifier, field, or array element. Hint: remove assignment to literals.”
- Bad operator arity: “Operator '.+( )' needs exactly one argument. Hint: write `x.+(value)`.”
- Struct locals: “Struct values live in globals; locals hold struct references (indices). Hint: store the index, not the struct.”

Process
- When adding diagnostics, include: what went wrong, where, and how to fix.
- Keep messages short (≤100 chars when possible); move detail to hints if needed.
- Reuse wording across parser/semantic/lowering for the same concept.
