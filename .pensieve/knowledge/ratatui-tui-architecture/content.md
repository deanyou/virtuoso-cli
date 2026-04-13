# ratatui TUI Architecture Pattern for CLI Tools

## Source
Implementing vcli TUI dashboard, referencing backCLI TUI design.

## Summary
Reusable architecture for adding ratatui TUI to an existing CLI tool: mod structure, event loop pattern, and key gotchas.

## Content

### Module Structure
```
src/tui/
├── mod.rs     — run_tui() entry + event loop
├── state.rs   — single TuiState struct (all app state)
├── input.rs   — KeyEvent → EventAction enum (NOT event.rs — collides with crossterm::event)
├── render.rs  — frame rendering (immediate mode, rebuild every frame)
└── theme.rs   — centralized colors
```

### Critical: Name your event handler `input.rs`, NOT `event.rs`
`crossterm::event` import collides with `mod event`. Causes `E0255: name defined multiple times`.

### Event Loop Pattern
```rust
loop {
    terminal.draw(|f| render(f, &state, &theme))?;
    if !crossterm::event::poll(Duration::from_millis(500))? {
        // idle tick: animate spinners, clear stale status msgs
        continue;
    }
    match crossterm::event::read()? {
        Event::Key(k) if k.kind == KeyEventKind::Press => {
            match input::handle_key(&mut state, k) {
                EventAction::Quit => break,
                // ...
            }
        }
        _ => {}
    }
}
```

### Don't refresh data on idle tick
Only refresh on explicit user action (r key). Idle tick should only update animations and clear stale messages.

### kv_line helper for detail panels
Use `Line<'static>` (owned Strings) to avoid lifetime issues with temporary `.to_string()` values:
```rust
fn kv_line(label: &str, value: &str, theme: &Theme, color: Option<Color>) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.to_string(), Style::default().fg(theme.text_dim)),
        Span::styled(value.to_string(), Style::default().fg(color.unwrap_or(theme.text))),
    ])
}
```

### CLI Integration
Add as both subcommand (`vcli tui`) and standalone binary (`vtui`).
vtui.rs just imports modules and calls `tui::run_tui()`.

## When to Use
- Adding TUI to any Rust CLI project
- Choosing between `Line<'a>` vs `Line<'static>` in ratatui render functions
- Structuring event handling to avoid crossterm name collisions
