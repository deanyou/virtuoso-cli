# SKILL sprintf: %d silently fails on float coordinates

## Source
Debugging `vcli schematic list-instances` — SKILL returned nil when using `%d` for `inst~>xy`.

## Summary
SKILL `sprintf` with `%d` on float values doesn't error — it silently returns nil, killing the entire `let` block.

## Content
Virtuoso schematic instance coordinates (`inst~>xy`) are float pairs `(0.0 3.0)`, not integers.
Using `sprintf(nil "%d" car(inst~>xy))` doesn't produce an error — the entire enclosing
`let()` expression evaluates to nil. No error message, no partial output.

**Fix**: Always use `%g` for coordinates in SKILL sprintf:
```skill
sprintf(nil "{\"x\":%g,\"y\":%g}" car(inst~>xy) cadr(inst~>xy))
```

## When to Use
- Writing SKILL that reads schematic/layout geometry coordinates
- Any SKILL sprintf where the data type might be float
- Debugging SKILL expressions that return nil with no error message
