//! Tip of the Day (`docs/design-project-panel.md` §4.5).
//!
//! The tips are data (`assets/tips.toml`, embedded). `{action:<id>}` in a tip
//! becomes the action's menu label with its current shortcut, and `{key:<id>}`
//! just the shortcut, so rebinding a key never leaves a stale tip.
//!
//! Which tip shows is random once per launch and never the one shown last
//! time. The randomness is only ever used to pick a tip: it never reaches a
//! project file or a render.

use std::sync::OnceLock;

use serde::Deserialize;

/// Tips longer than this (after substitution) are rejected by the validator.
#[cfg(test)]
pub const MAX_LEN: usize = 160;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Tip {
    pub id: String,
    pub text: String,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    tip: Vec<Tip>,
}

/// Parses a tips file.
pub fn parse(source: &str) -> Result<Vec<Tip>, String> {
    toml::from_str::<File>(source)
        .map(|f| f.tip)
        .map_err(|e| e.to_string())
}

/// The built-in tips. A broken embedded file (a build error caught by the
/// tests) yields no tips rather than a crash.
pub fn all() -> &'static [Tip] {
    static TIPS: OnceLock<Vec<Tip>> = OnceLock::new();
    TIPS.get_or_init(|| match parse(include_str!("../assets/tips.toml")) {
        Ok(tips) => tips,
        Err(e) => {
            tracing::error!("the built-in tips could not be read: {e}");
            Vec::new()
        }
    })
}

/// A placeholder in a tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token<'a> {
    /// `{action:<id>}`: label and shortcut.
    Action(&'a str),
    /// `{key:<id>}`: the shortcut only.
    Key(&'a str),
}

/// Splits `text` into literal runs and placeholders. A `{` that does not
/// open a known placeholder is an error, so a typo cannot reach the screen.
pub fn tokens(text: &str) -> Result<Vec<Result<Token<'_>, &str>>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find(['{', '}']) {
        if open > 0 {
            out.push(Err(&rest[..open]));
        }
        let tail = &rest[open..];
        if !tail.starts_with('{') {
            return Err(format!("a stray '}}' in {text:?}"));
        }
        let Some(close) = tail.find('}') else {
            return Err(format!("an unclosed '{{' in {text:?}"));
        };
        let inner = &tail[1..close];
        let token = match inner.split_once(':') {
            Some(("action", id)) if !id.is_empty() => Token::Action(id),
            Some(("key", id)) if !id.is_empty() => Token::Key(id),
            _ => return Err(format!("unknown placeholder {{{inner}}} in {text:?}")),
        };
        out.push(Ok(token));
        rest = &tail[close + 1..];
    }
    if !rest.is_empty() {
        out.push(Err(rest));
    }
    Ok(out)
}

/// What a placeholder resolves to: `(label, shortcut)` of an action, or
/// `None` when the action does not exist.
pub type Resolver<'a> = dyn Fn(&str) -> Option<(String, Option<String>)> + 'a;

/// The tip's text with every placeholder replaced.
pub fn render(text: &str, resolve: &Resolver<'_>) -> Result<String, String> {
    let mut out = String::new();
    for part in tokens(text)? {
        match part {
            Err(literal) => out.push_str(literal),
            Ok(Token::Action(id)) => {
                let (label, key) = resolve(id).ok_or_else(|| format!("no action {id:?}"))?;
                out.push_str(&label);
                if let Some(key) = key {
                    out.push_str(" (");
                    out.push_str(&key);
                    out.push(')');
                }
            }
            Ok(Token::Key(id)) => {
                let (label, key) = resolve(id).ok_or_else(|| format!("no action {id:?}"))?;
                // An action with no shortcut falls back to its name, so the
                // sentence still reads.
                out.push_str(&key.unwrap_or(label));
            }
        }
    }
    Ok(out)
}

/// A small, fast, well-spread hash step (splitmix64), so nearby seeds give
/// unrelated picks.
fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The index of the tip for this launch: drawn from every tip except the one
/// shown last time (unless it is the only one). Pure; tests pass fixed seeds.
pub fn pick_tip(tips: &[Tip], last: Option<&str>, seed: u64) -> Option<usize> {
    if tips.is_empty() {
        return None;
    }
    let candidates: Vec<usize> = (0..tips.len())
        .filter(|&i| Some(tips[i].id.as_str()) != last)
        .collect();
    if candidates.is_empty() {
        return Some(0);
    }
    let n = candidates.len() as u64;
    Some(candidates[(mix(seed) % n) as usize])
}

/// "Next tip": the following tip in file order, wrapping.
pub fn next_tip(tips: &[Tip], current: usize) -> Option<usize> {
    if tips.is_empty() {
        None
    } else {
        Some((current + 1) % tips.len())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use egui::{Context, FontId, RawInput, text::LayoutJob};

    use super::*;
    use crate::actions::{Action, Keymap};

    /// Resolves ids the way the app does, with default shortcuts.
    fn resolver(ctx: &Context) -> impl Fn(&str) -> Option<(String, Option<String>)> + '_ {
        let keymap = Keymap::default();
        move |id| {
            let action = Action::from_id(id)?;
            let key = keymap
                .bindings(action)
                .first()
                .map(|s| ctx.format_shortcut(s));
            Some((action.label().to_owned(), key))
        }
    }

    fn tip(id: &str) -> Tip {
        Tip {
            id: id.into(),
            text: id.into(),
        }
    }

    #[test]
    fn there_are_at_least_thirty_tips_with_unique_ids_and_text() {
        let tips = all();
        assert!(tips.len() >= 30, "{}", tips.len());
        let ids: BTreeSet<_> = tips.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.len(), tips.len(), "ids must be unique");
        for t in tips {
            assert!(
                !t.id.trim().is_empty() && !t.text.trim().is_empty(),
                "{t:?}"
            );
        }
    }

    #[test]
    fn every_placeholder_names_a_real_action_and_nothing_raw_is_left() {
        let ctx = Context::default();
        let resolve = resolver(&ctx);
        for t in all() {
            let text = render(&t.text, &resolve).unwrap_or_else(|e| panic!("{}: {e}", t.id));
            assert!(!text.contains(['{', '}']), "{}: {text}", t.id);
            assert!(
                !text.contains("action:") && !text.contains("key:"),
                "{}",
                t.id
            );
        }
    }

    #[test]
    fn key_placeholders_name_actions_that_have_a_default_shortcut() {
        let keymap = Keymap::default();
        for t in all() {
            for part in tokens(&t.text).unwrap() {
                if let Ok(Token::Key(id)) = part {
                    let action = Action::from_id(id).unwrap_or_else(|| panic!("{}", t.id));
                    assert!(
                        !keymap.bindings(action).is_empty(),
                        "{}: {id} has no default shortcut",
                        t.id
                    );
                }
            }
        }
    }

    #[test]
    fn no_tip_is_longer_than_the_limit_after_substitution() {
        let ctx = Context::default();
        let resolve = resolver(&ctx);
        for t in all() {
            let text = render(&t.text, &resolve).unwrap();
            assert!(
                text.chars().count() <= MAX_LEN,
                "{} is {} characters: {text}",
                t.id,
                text.chars().count()
            );
        }
    }

    #[test]
    fn every_tip_fits_two_lines_at_the_narrowest_tile_width() {
        let ctx = Context::default();
        let resolve = resolver(&ctx);
        let mut out = ctx.run_ui(
            RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..RawInput::default()
            },
            |ui| {
                // Text is laid out at the width of the narrowest tile layout
                // (320 points) as the screen draws it.
                let mut too_long = Vec::new();
                for t in all() {
                    let text = render(&t.text, &resolve).unwrap();
                    let mut job = LayoutJob::single_section(
                        text.clone(),
                        egui::TextFormat::simple(FontId::proportional(13.0), egui::Color32::WHITE),
                    );
                    job.wrap.max_width = crate::empty_state::TIP_TEXT_WIDTH;
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    if galley.rows.len() > 2 {
                        too_long.push(format!("{} ({} lines): {text}", t.id, galley.rows.len()));
                    }
                }
                assert!(too_long.is_empty(), "{}", too_long.join("\n"));
            },
        );
        out.textures_delta.clear();
    }

    #[test]
    fn a_rebound_key_shows_in_the_tip() {
        let resolve =
            |id: &str| (id == "x").then(|| ("Label".to_owned(), Some("Ctrl+9".to_owned())));
        assert_eq!(
            render("Use {action:x} now", &resolve).unwrap(),
            "Use Label (Ctrl+9) now"
        );
        assert_eq!(render("Press {key:x}.", &resolve).unwrap(), "Press Ctrl+9.");
        let unbound = |_: &str| Some(("Label".to_owned(), None));
        assert_eq!(render("{action:x}", &unbound).unwrap(), "Label");
        assert_eq!(render("{key:x}", &unbound).unwrap(), "Label");
    }

    #[test]
    fn broken_placeholders_are_errors() {
        let any = |_: &str| Some((String::new(), None));
        for bad in ["{oops}", "{action:}", "{action:x", "stray }", "{act:x}"] {
            assert!(render(bad, &any).is_err(), "{bad}");
        }
        let none = |_: &str| None;
        assert!(render("{action:missing}", &none).is_err());
    }

    #[test]
    fn the_pick_is_never_the_last_tip_and_is_reproducible() {
        let tips: Vec<Tip> = (0..5).map(|i| tip(&format!("t{i}"))).collect();
        for seed in 0..500u64 {
            for last in [None, Some("t0"), Some("t3")] {
                let i = pick_tip(&tips, last, seed).unwrap();
                assert_ne!(Some(tips[i].id.as_str()), last, "seed {seed}");
                assert_eq!(pick_tip(&tips, last, seed), Some(i), "same seed, same tip");
            }
        }
    }

    #[test]
    fn the_pick_reaches_every_other_tip() {
        let tips: Vec<Tip> = (0..5).map(|i| tip(&format!("t{i}"))).collect();
        let seen: BTreeSet<usize> = (0..400u64)
            .filter_map(|s| pick_tip(&tips, Some("t2"), s))
            .collect();
        assert_eq!(seen, BTreeSet::from([0, 1, 3, 4]));
    }

    #[test]
    fn a_single_tip_is_shown_even_if_it_was_last() {
        let tips = vec![tip("only")];
        assert_eq!(pick_tip(&tips, Some("only"), 7), Some(0));
        assert_eq!(pick_tip(&[], None, 7), None);
    }

    #[test]
    fn next_tip_follows_file_order_and_wraps() {
        let tips: Vec<Tip> = (0..3).map(|i| tip(&format!("t{i}"))).collect();
        assert_eq!(next_tip(&tips, 0), Some(1));
        assert_eq!(next_tip(&tips, 2), Some(0));
        assert_eq!(next_tip(&[], 0), None);
    }
}
