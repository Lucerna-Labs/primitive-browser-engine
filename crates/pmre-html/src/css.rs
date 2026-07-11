//! A minimal CSS engine: `<style>` block parsing into rules, and selector
//! matching against an element's tag/id/class-list — with descendant
//! combinator support (`div p`, `article .card p`). Bounded scope, matching
//! `html.rs`'s own doctrine — simple and compound selectors (type, `.class`,
//! `#id`, and combinations of those on *one* element, e.g. `div.card#hero`),
//! multi-compound chains joined by descendant combinators, and a basic
//! specificity-ordered cascade. No child (`>`) / sibling (`+` / `~`) combinators,
//! no pseudo-classes/attribute selectors/universal `*`, no `@media`/`@import` —
//! a selector using any of that unsupported syntax is dropped (the entire rule
//! matches nothing) rather than mis-parsed into matching a broader or narrower
//! set of elements than the author intended.

/// One compound selector: every part must match the same element.
/// `div.card#hero` → `{ type_name: Some("div"), id: Some("hero"), classes: ["card"] }`.
#[derive(Clone, Debug, Default)]
pub struct Selector {
    pub type_name: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
}

impl Selector {
    /// CSS specificity as `(id, class, type)` counts, compared
    /// lexicographically — matches the real cascade's id > class > type
    /// precedence order exactly (tuple comparison is lexicographic in Rust).
    fn specificity(&self) -> (u32, u32, u32) {
        (
            u32::from(self.id.is_some()),
            self.classes.len() as u32,
            u32::from(self.type_name.is_some()),
        )
    }

    fn matches(&self, tag: &str, id: Option<&str>, classes: &[&str]) -> bool {
        if let Some(t) = &self.type_name {
            if t != tag {
                return false;
            }
        }
        if let Some(want) = &self.id {
            if id != Some(want.as_str()) {
                return false;
            }
        }
        self.classes.iter().all(|c| classes.contains(&c.as_str()))
    }
}

/// A descendant chain: one or more compound `Selector`s to be matched
/// right-to-left. `[Selector { type_name: Some("div") }, Selector {
/// type_name: Some("p") }]` matches a `<p>` that has some ancestor `<div>`.
/// A single-compound chain (length 1) behaves like the old direct-match
/// selector — no ancestor requirement — so nothing about pre-descendant
/// stylesheets changes.
pub type Chain = Vec<Selector>;

/// One parsed rule: a comma-separated selector list (matches if *any* chain
/// in the list matches) plus its declaration block, kept as the raw
/// `prop: value; ...` text. A `<style>` rule's body and a `style="..."`
/// attribute are the same grammar — `crate::html`'s existing `apply_css` (a
/// complete, tested inline-style parser) is reused verbatim for rule bodies
/// too, reached from different syntax rather than reimplemented.
pub struct Rule {
    chains: Vec<Chain>,
    pub declarations: String,
    /// Position in the stylesheet — later rules of equal specificity win,
    /// the same source-order tiebreak the real cascade uses.
    pub order: usize,
}

/// One (tag, id, classes) frame in the ancestor chain a descendant selector
/// walks up. The kit's HTML reducer pushes one of these into a &mut Vec
/// before recursing into an element's children and pops it after — so the
/// slice passed here is always the parent chain of the currently-matched
/// element, root-first.
pub type AncestorFrame<'a> = (&'a str, Option<&'a str>, &'a [&'a str]);

impl Rule {
    /// The highest specificity among this rule's chains that matches the
    /// given element (in the context of its ancestor chain), or `None` if
    /// none of them do.
    pub fn specificity_if_matches(
        &self,
        ancestors: &[AncestorFrame<'_>],
        tag: &str,
        id: Option<&str>,
        classes: &[&str],
    ) -> Option<(u32, u32, u32)> {
        self.chains
            .iter()
            .filter_map(|chain| match_chain(chain, ancestors, tag, id, classes))
            .max()
    }
}

/// Match a chain against an element in the context of its ancestor chain.
/// Returns the chain's summed specificity if it matches, `None` otherwise.
///
/// Algorithm: the last (rightmost) compound must match the element itself.
/// Preceding compounds must each match *some* ancestor, in order — walking
/// backwards from the nearest ancestor toward the root, and each preceding
/// compound's matching ancestor must be strictly earlier (higher up the
/// tree) than the next compound's. This is the standard CSS descendant
/// combinator semantics.
fn match_chain(
    chain: &[Selector],
    ancestors: &[AncestorFrame<'_>],
    tag: &str,
    id: Option<&str>,
    classes: &[&str],
) -> Option<(u32, u32, u32)> {
    let (last, rest) = chain.split_last()?;
    if !last.matches(tag, id, classes) {
        return None;
    }
    // Iterate the preceding compounds from rightmost (nearest ancestor)
    // toward the root, and walk the ancestor slice from its end (nearest
    // to the element) backward — each compound must find some ancestor
    // strictly earlier than the previous compound's match.
    let mut ancestor_i = ancestors.len();
    for step in rest.iter().rev() {
        let mut found = false;
        while ancestor_i > 0 {
            ancestor_i -= 1;
            let (at, aid, acls) = ancestors[ancestor_i];
            if step.matches(at, aid, acls) {
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }
    // Specificity of a chain is the field-wise sum of its compound
    // specificities, per the real CSS cascade (a chain with two class
    // selectors has specificity (0, 2, 0), not (0, 1, 0)).
    let mut spec = (0u32, 0u32, 0u32);
    for c in chain {
        let s = c.specificity();
        spec.0 += s.0;
        spec.1 += s.1;
        spec.2 += s.2;
    }
    Some(spec)
}

/// Parse a stylesheet's combined text (the concatenation of every `<style>`
/// block's content) into an ordered rule list. Tolerant: a chunk that
/// doesn't parse as `selectors { declarations }` is skipped, never panics.
pub fn parse_stylesheet(css: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut order = 0usize;
    let mut i = 0usize;
    let bytes = css.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if css[i..].starts_with("/*") {
            i = css[i + 2..]
                .find("*/")
                .map(|end| i + 2 + end + 2)
                .unwrap_or(css.len());
            continue;
        }
        let Some(brace) = css[i..].find('{') else {
            break; // trailing garbage after the last rule — stop cleanly
        };
        let selector_text = &css[i..i + brace];
        let Some(close_rel) = css[i + brace + 1..].find('}') else {
            break; // unterminated block — stop cleanly
        };
        let decl_start = i + brace + 1;
        let decl_end = decl_start + close_rel;
        let chains: Vec<Chain> = selector_text
            .split(',')
            .filter_map(parse_selector)
            .collect();
        if !chains.is_empty() {
            rules.push(Rule {
                chains,
                declarations: css[decl_start..decl_end].to_string(),
                order,
            });
            order += 1;
        }
        i = decl_end + 1;
    }
    rules
}

/// Parse one comma-separated selector as a descendant chain — one or more
/// whitespace-separated compound selectors. Returns `None` (fail-closed) if
/// any compound uses unsupported syntax (`>`, `+`, `~`, `*`, `[`, `:`), so
/// a rule like `div > p` matches nothing rather than degrading into `div p`.
fn parse_selector(s: &str) -> Option<Chain> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Reject unsupported combinators / selector syntax at the WHOLE selector
    // level — anywhere they appear inside a chain fails the whole chain.
    if s.contains(['>', '+', '~', '*', '[', ':']) {
        return None;
    }
    let mut chain: Chain = Vec::new();
    for part in s.split_whitespace() {
        let sel = parse_compound(part)?;
        chain.push(sel);
    }
    if chain.is_empty() {
        None
    } else {
        Some(chain)
    }
}

/// Parse one compound selector (no whitespace, no combinators) — the
/// smallest unit inside a descendant chain.
fn parse_compound(s: &str) -> Option<Selector> {
    if s.is_empty() {
        return None;
    }
    let mut sel = Selector::default();
    let mut cur = String::new();
    let mut kind = 'e'; // 'e' = type/tag, '.' = class, '#' = id
    for ch in s.chars() {
        if ch == '.' || ch == '#' {
            set_part(&mut sel, kind, &cur);
            cur.clear();
            kind = ch;
        } else {
            cur.push(ch);
        }
    }
    set_part(&mut sel, kind, &cur);
    if sel.type_name.is_none() && sel.id.is_none() && sel.classes.is_empty() {
        None
    } else {
        Some(sel)
    }
}

fn set_part(sel: &mut Selector, kind: char, part: &str) {
    if part.is_empty() {
        return;
    }
    match kind {
        '.' => sel.classes.push(part.to_string()),
        '#' => sel.id = Some(part.to_string()),
        _ => sel.type_name = Some(part.to_ascii_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_ancestors() -> Vec<AncestorFrame<'static>> {
        Vec::new()
    }

    #[test]
    fn parses_type_class_and_id_selectors() {
        let rules =
            parse_stylesheet("div { color: red; } .card { color: blue; } #hero { color: green; }");
        assert_eq!(rules.len(), 3);
        let a = no_ancestors();
        assert!(rules[0]
            .specificity_if_matches(&a, "div", None, &[])
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(&a, "span", None, &[])
            .is_none());
        assert!(rules[1]
            .specificity_if_matches(&a, "div", None, &["card"])
            .is_some());
        assert!(rules[1]
            .specificity_if_matches(&a, "div", None, &[])
            .is_none());
        assert!(rules[2]
            .specificity_if_matches(&a, "div", Some("hero"), &[])
            .is_some());
    }

    #[test]
    fn compound_selector_requires_every_part() {
        let rules = parse_stylesheet("div.card#hero { color: red; }");
        assert_eq!(rules.len(), 1);
        let a = no_ancestors();
        assert!(rules[0]
            .specificity_if_matches(&a, "div", Some("hero"), &["card"])
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(&a, "div", Some("hero"), &[])
            .is_none());
        assert!(rules[0]
            .specificity_if_matches(&a, "span", Some("hero"), &["card"])
            .is_none());
    }

    #[test]
    fn comma_separated_selector_list_matches_either() {
        let rules = parse_stylesheet("h1, h2 { color: red; }");
        assert_eq!(rules.len(), 1);
        let a = no_ancestors();
        assert!(rules[0]
            .specificity_if_matches(&a, "h1", None, &[])
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(&a, "h2", None, &[])
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(&a, "h3", None, &[])
            .is_none());
    }

    #[test]
    fn id_beats_class_beats_type_specificity() {
        let type_sel = parse_compound("div").unwrap();
        let class_sel = parse_compound(".card").unwrap();
        let id_sel = parse_compound("#hero").unwrap();
        assert!(id_sel.specificity() > class_sel.specificity());
        assert!(class_sel.specificity() > type_sel.specificity());
    }

    #[test]
    fn descendant_combinator_matches_when_ancestor_matches() {
        // `div p` matches a <p> whose ancestors include a <div>.
        let rules = parse_stylesheet("div p { color: red; }");
        assert_eq!(rules.len(), 1);
        let empty: &[&str] = &[];
        let ancestors: Vec<AncestorFrame<'_>> = vec![("div", None, empty)];
        assert!(
            rules[0]
                .specificity_if_matches(&ancestors, "p", None, &[])
                .is_some(),
            "div p should match a p descendant of a div"
        );
        // Without a div ancestor: no match.
        let no_ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        assert!(rules[0]
            .specificity_if_matches(&no_ancestors, "p", None, &[])
            .is_none());
        // p without a div ancestor (say inside an article) doesn't match.
        let article: Vec<AncestorFrame<'_>> = vec![("article", None, empty)];
        assert!(rules[0]
            .specificity_if_matches(&article, "p", None, &[])
            .is_none());
    }

    #[test]
    fn descendant_combinator_can_skip_intermediate_ancestors() {
        // `article p` must match a p inside an article regardless of any
        // section/div wrappers between them.
        let rules = parse_stylesheet("article p { color: red; }");
        let empty: &[&str] = &[];
        let ancestors: Vec<AncestorFrame<'_>> = vec![
            ("article", None, empty),
            ("section", None, empty),
            ("div", None, empty),
        ];
        assert!(rules[0]
            .specificity_if_matches(&ancestors, "p", None, &[])
            .is_some());
    }

    #[test]
    fn three_step_chain_requires_matches_in_root_to_leaf_order() {
        // `html body p` matches a p inside a body inside an html.
        let rules = parse_stylesheet("html body p { color: red; }");
        let empty: &[&str] = &[];
        let good: Vec<AncestorFrame<'_>> = vec![("html", None, empty), ("body", None, empty)];
        assert!(rules[0]
            .specificity_if_matches(&good, "p", None, &[])
            .is_some());
        // Wrong order (body then html) shouldn't match.
        let reversed: Vec<AncestorFrame<'_>> = vec![("body", None, empty), ("html", None, empty)];
        assert!(rules[0]
            .specificity_if_matches(&reversed, "p", None, &[])
            .is_none());
    }

    #[test]
    fn descendant_chain_specificity_sums_the_compounds() {
        let rules = parse_stylesheet(".card p { color: red; }");
        let ancestors: Vec<AncestorFrame<'_>> = vec![("div", None, &["card"])];
        let spec = rules[0]
            .specificity_if_matches(&ancestors, "p", None, &[])
            .expect("should match");
        // (0, 1, 1): one class from `.card`, one type from `p`.
        assert_eq!(spec, (0, 1, 1));
    }

    #[test]
    fn unsupported_combinator_syntax_still_fails_closed() {
        // `>` is a child combinator — still unsupported, whole rule dropped.
        let rules = parse_stylesheet("div > p { color: red; }");
        assert_eq!(
            rules.len(),
            0,
            "child combinator should still be dropped (not supported)"
        );
        // `+`, `~`, `*`, `[`, `:` also dropped.
        assert_eq!(parse_stylesheet("a:hover { color: red; }").len(), 0);
        assert_eq!(parse_stylesheet("div + p { color: red; }").len(), 0);
        assert_eq!(parse_stylesheet("div ~ p { color: red; }").len(), 0);
        assert_eq!(parse_stylesheet("[data-x] { color: red; }").len(), 0);
        assert_eq!(parse_stylesheet("* { color: red; }").len(), 0);
    }

    #[test]
    fn malformed_trailing_text_does_not_panic() {
        let rules = parse_stylesheet("div { color: red; } trailing garbage no brace");
        assert_eq!(rules.len(), 1);
    }
}
