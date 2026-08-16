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

/// An attribute selector constraint on a single element, e.g.
/// `[href]`, `[href="/x"]`, `[href^="https"]`, `[lang|=en]`. The kit's HTML
/// reducer doesn't thread the full attribute map to the cascade (only
/// tag/id/class today), so attribute selectors match against the *known*
/// attributes the reducer does expose: `id` (via the id matcher) and the
/// element's `class` list. Unknown attributes — the common case — fail
/// closed (no match) rather than mis-match, since the reducer has no opinion
/// on them. This keeps `[id]` / `[class]` working today and leaves the door
/// open for a richer attribute plumbing later.
#[derive(Clone, Debug)]
pub struct AttrSelector {
    pub name: String,
    pub op: AttrOp,
    pub value: Option<String>,
}

/// The comparison an [`AttrSelector`] applies (CSS attr-selector operators).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrOp {
    /// `[attr]` — attribute is present.
    Exists,
    /// `[attr=val]` — exactly equal.
    Equals,
    /// `[attr^=val]` — begins with.
    Prefix,
    /// `[attr$=val]` — ends with.
    Suffix,
    /// `[attr*=val]` — contains.
    Contains,
}

/// One compound selector: every part must match the same element.
/// `div.card#hero` → `{ type_name: Some("div"), id: Some("hero"), classes: ["card"] }`.
/// A pseudo-class constraint on an element (`:hover`, `:focus`). Matched
/// against the element's interaction state — whether it is the currently
/// hovered/focused element, supplied to the cascade by the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pseudo {
    /// No pseudo-class (the common case).
    #[default]
    None,
    /// `:hover` — the element is the currently-hovered one.
    Hover,
    /// `:focus` — the element is the currently-focused one.
    Focus,
}

/// The interaction state the browser supplies to the cascade: the id of the
/// currently hovered element and the currently focused element (if any). Used
/// to match `:hover` / `:focus` pseudo-classes. Passed through `match_chain`
/// alongside the ancestor/sibling slices.
#[derive(Clone, Copy, Debug, Default)]
pub struct InteractionState {
    pub hovered_id: Option<&'static str>,
    pub focused_id: Option<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct Selector {
    pub type_name: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    /// Attribute selectors (`[href]`, `[class^="btn"]`, …). Empty for the
    /// common case. Matched against id/class only today (see [`AttrSelector`]).
    pub attrs: Vec<AttrSelector>,
    /// A pseudo-class (`:hover`, `:focus`) on this compound, if any.
    pub pseudo: Pseudo,
}

impl Selector {
    /// CSS specificity as `(id, class, type)` counts, compared
    /// lexicographically — matches the real cascade's id > class > type
    /// precedence order exactly (tuple comparison is lexicographic in Rust).
    /// Attribute selectors count as class-level specificity (one each), per
    /// the CSS spec.
    fn specificity(&self) -> (u32, u32, u32) {
        (
            u32::from(self.id.is_some()),
            self.classes.len() as u32 + self.attrs.len() as u32,
            u32::from(self.type_name.is_some()),
        )
    }

    fn matches(
        &self,
        tag: &str,
        id: Option<&str>,
        classes: &[&str],
        interaction: InteractionState,
    ) -> bool {
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
        if !self.classes.iter().all(|c| classes.contains(&c.as_str())) {
            return false;
        }
        // Attribute selectors: match against id / class only today.
        if !self.attrs.iter().all(|a| a.matches(id, classes)) {
            return false;
        }
        // Pseudo-class: the element must be the hovered/focused one.
        match self.pseudo {
            Pseudo::None => true,
            Pseudo::Hover => id.is_some() && interaction.hovered_id == id,
            Pseudo::Focus => id.is_some() && interaction.focused_id == id,
        }
    }
}

impl AttrSelector {
    /// Match this attribute selector against the element's known attributes
    /// (id + class list, the only ones the reducer exposes today). Unknown
    /// attribute names fail closed (no match).
    fn matches(&self, id: Option<&str>, classes: &[&str]) -> bool {
        // Resolve the attribute's current value, if it's one we track.
        let val: Option<String> = if self.name.eq_ignore_ascii_case("id") {
            id.map(str::to_string)
        } else if self.name.eq_ignore_ascii_case("class") {
            let v = classes.join(" ");
            Some(v)
        } else {
            // Unknown attribute — the reducer doesn't expose it. Fail closed.
            None
        };
        match self.op {
            AttrOp::Exists => val.is_some(),
            AttrOp::Equals => val.as_deref() == self.value.as_deref(),
            AttrOp::Prefix => val
                .as_deref()
                .zip(self.value.as_deref())
                .map(|(v, w)| v.starts_with(w))
                .unwrap_or(false),
            AttrOp::Suffix => val
                .as_deref()
                .zip(self.value.as_deref())
                .map(|(v, w)| v.ends_with(w))
                .unwrap_or(false),
            AttrOp::Contains => val
                .as_deref()
                .zip(self.value.as_deref())
                .map(|(v, w)| v.contains(w))
                .unwrap_or(false),
        }
    }
}

/// The combinator joining one compound selector to the preceding step in a
/// chain. Descendant (whitespace) is the default and the historical
/// behaviour; `>` (child) is new; `+`/`~` (siblings) need sibling context the
/// reducer doesn't thread yet, so they still fail closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Combinator {
    /// `A B` — B is a descendant of A (any depth). The default; pre-existing.
    #[default]
    Descendant,
    /// `A > B` — B is a direct child of A.
    Child,
    /// `A + B` — B immediately follows sibling A (same parent, adjacent).
    AdjacentSibling,
    /// `A ~ B` — B follows sibling A (same parent, any later position).
    GeneralSibling,
}

/// One step in a selector chain: a compound selector plus the combinator
/// that joins it to the step *before* it (the leftmost step's combinator is
/// unused — there's nothing to its left). Replaces the bare `Vec<Selector>`
/// chain so child combinators carry their own semantics.
#[derive(Clone, Debug)]
pub struct ChainStep {
    pub selector: Selector,
    pub combinator: Combinator,
}

/// A selector chain: one or more [`ChainStep`]s matched right-to-left.
/// `[Child(div) Descendant(p)]` matches a `<p>` whose parent chain contains a
/// `<div>` (with the `<p>` itself matched by the rightmost step). A
/// single-step chain behaves like the old direct-match selector.
pub type Chain = Vec<ChainStep>;

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
        siblings: &[AncestorFrame<'_>],
        interaction: InteractionState,
        tag: &str,
        id: Option<&str>,
        classes: &[&str],
    ) -> Option<(u32, u32, u32)> {
        self.chains
            .iter()
            .filter_map(|chain| {
                match_chain(chain, ancestors, siblings, interaction, tag, id, classes)
            })
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
    chain: &[ChainStep],
    ancestors: &[AncestorFrame<'_>],
    siblings: &[AncestorFrame<'_>],
    interaction: InteractionState,
    tag: &str,
    id: Option<&str>,
    classes: &[&str],
) -> Option<(u32, u32, u32)> {
    // Walk the chain right-to-left (element first, then its ancestors/siblings).
    // The combinator that governs the relationship between step `i` and the
    // element/step to its *right* lives on step `i+1` (or `last`). So when
    // matching an ancestor/sibling for step `rest[i]`, we consult the
    // combinator of the step we just matched toward.
    let (last, rest) = chain.split_last()?;
    if !last.selector.matches(tag, id, classes, interaction) {
        return None;
    }
    // `ancestor_i` is the index *after* the most-recently-considered
    // ancestor: ancestors[..ancestor_i] remain as candidates for the next
    // step up. Start at the end (the element's nearest ancestor).
    let mut ancestor_i = ancestors.len();
    // `sibling_i` tracks where we are in the preceding-siblings slice
    // (nearest-last: siblings[siblings.len()-1] is the element's immediate
    // preceding sibling). Sibling combinators only ever consult the
    // immediate-or-near preceding siblings and then hand off to ancestor
    // matching for the next combinator up.
    let mut sibling_i = siblings.len();
    // Walk rest right-to-left; for each step, the combinator that applies is
    // the one on the *following* (already-matched, rightward) step.
    let mut following_combinator = last.combinator;
    for step in rest.iter().rev() {
        let mut found = false;
        match following_combinator {
            Combinator::Descendant => {
                // Any earlier ancestor can match.
                while ancestor_i > 0 {
                    ancestor_i -= 1;
                    let (at, aid, acls) = ancestors[ancestor_i];
                    if step.selector.matches(at, aid, acls, interaction) {
                        found = true;
                        break;
                    }
                }
            }
            Combinator::Child => {
                // Only the immediate parent (ancestor_i - 1) can satisfy a
                // child combinator. If it doesn't match, the whole chain
                // fails — no walking further up.
                if ancestor_i == 0 {
                    return None;
                }
                ancestor_i -= 1;
                let (at, aid, acls) = ancestors[ancestor_i];
                if step.selector.matches(at, aid, acls, interaction) {
                    found = true;
                }
            }
            Combinator::AdjacentSibling => {
                // A + B: B's immediate preceding sibling must be A. The
                // siblings slice is nearest-last, so siblings[len-1] is the
                // immediate predecessor. If it doesn't match, fail.
                if sibling_i == 0 {
                    return None;
                }
                sibling_i -= 1;
                let (st, sid, scls) = siblings[sibling_i];
                if step.selector.matches(st, sid, scls, interaction) {
                    found = true;
                }
            }
            Combinator::GeneralSibling => {
                // A ~ B: some preceding sibling must be A. Walk back through
                // the earlier siblings until one matches.
                while sibling_i > 0 {
                    sibling_i -= 1;
                    let (st, sid, scls) = siblings[sibling_i];
                    if step.selector.matches(st, sid, scls, interaction) {
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found {
            return None;
        }
        following_combinator = step.combinator;
    }
    // Specificity of a chain is the field-wise sum of its compound
    // specificities, per the real CSS cascade (a chain with two class
    // selectors has specificity (0, 2, 0), not (0, 1, 0)).
    let mut spec = (0u32, 0u32, 0u32);
    for c in chain {
        let s = c.selector.specificity();
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

/// Parse one comma-separated selector as a chain — one or more
/// whitespace-separated compound selectors joined by combinators. Supported
/// combinators: descendant (whitespace) and child (`>`). Still fail-closed on
/// sibling combinators (`+`/`~`), universal (`*`), and pseudo-classes (`:`) —
/// they need context the reducer doesn't thread (sibling stack, interaction
/// state), so a rule using them matches nothing rather than mis-matching.
/// Attribute selectors (`[attr]`, `[attr=val]`, …) are supported via
/// [`parse_compound`].
fn parse_selector(s: &str) -> Option<Chain> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Universal `*` still fails closed (matches everything — not useful
    // against tag/id/class today). Pseudo-classes (`:hover`/`:focus`) are
    // parsed + matched by parse_compound / Selector::matches.
    if s.contains('*') {
        return None;
    }
    // Tokenise: split on whitespace, but keep standalone combinators (`>`,
    // `+`, `~`) as their own tokens so `div > p` yields [div, >, p]. Multiple
    // spaces collapse via split_whitespace already.
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut chain: Chain = Vec::new();
    // The combinator pending for the *next* compound (default Descendant).
    let mut pending = Combinator::Descendant;
    for tok in tokens {
        // A standalone combinator token sets the pending combinator.
        if matches!(tok, ">" | "+" | "~") {
            pending = match tok {
                ">" => Combinator::Child,
                "+" => Combinator::AdjacentSibling,
                "~" => Combinator::GeneralSibling,
                _ => unreachable!(),
            };
            continue;
        }
        // A compound may contain `>` with no spaces (`div>p`); handle that by
        // splitting on `>` so each sub gets the Child combinator. (`+`/`~`
        // without spaces like `div+p` are also handled by the same split.)
        let mut sub_iter = tok.split(['>', '+', '~']).peekable();
        let mut first_sub = true;
        while let Some(sub) = sub_iter.next() {
            if !first_sub {
                // A `>`/`+`/`~` inside the token: pick the combinator by
                // scanning the original token for which one separates here.
                // The split consumed one char; find it to know which.
                pending = if tok.contains('>') {
                    Combinator::Child
                } else if tok.contains('+') {
                    Combinator::AdjacentSibling
                } else {
                    Combinator::GeneralSibling
                };
            }
            if sub.is_empty() {
                // leading/trailing combinator inside the token (e.g. `div>` or `>p`)
                first_sub = false;
                continue;
            }
            let sel = parse_compound(sub)?;
            // The first compound's combinator is whatever was pending before
            // it; subsequent compounds in this token carry the inner combinator.
            chain.push(ChainStep {
                selector: sel,
                combinator: pending,
            });
            pending = Combinator::Descendant;
            first_sub = false;
            // If there's another sub after this one (split on a combinator),
            // the next carries that combinator.
            if sub_iter.peek().is_some() {
                pending = if tok.contains('>') {
                    Combinator::Child
                } else if tok.contains('+') {
                    Combinator::AdjacentSibling
                } else {
                    Combinator::GeneralSibling
                };
            }
        }
    }
    if chain.is_empty() {
        None
    } else {
        Some(chain)
    }
}

/// Parse one compound selector (no whitespace, no combinators) — the
/// smallest unit inside a chain. Supports type/`.class`/`#id` plus attribute
/// selectors (`[attr]`, `[attr=val]`, `[attr^=val]`, `[attr$=val]`,
/// `[attr*=val]`). Quoted values (`[href="x"]`) and unquoted (`[href=x]`)
/// both work.
fn parse_compound(s: &str) -> Option<Selector> {
    if s.is_empty() {
        return None;
    }
    let mut sel = Selector::default();
    // First, peel off any `[...]` attribute selectors so they don't confuse
    // the `.`/`#` walk below. A `[` without a matching `]` fails the whole
    // compound (fail-closed).
    let mut rest = s.to_string();
    while let Some(open) = rest.find('[') {
        let Some(close_rel) = rest[open..].find(']') else {
            return None; // unterminated attribute selector
        };
        let close = open + close_rel;
        let attr_text = &rest[open + 1..close];
        let attr = parse_attr(attr_text)?; // malformed attribute selector -> None
        sel.attrs.push(attr);
        rest = format!("{}{}", &rest[..open], &rest[close + 1..]);
    }

    // Peel off any `:hover` / `:focus` pseudo-classes. Only these two are
    // supported; an unknown pseudo-class fails the whole compound closed.
    while let Some(colon) = rest.find(':') {
        let after = &rest[colon + 1..];
        let end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .unwrap_or(after.len());
        let name = &after[..end];
        let pseudo = match name {
            "hover" => Pseudo::Hover,
            "focus" => Pseudo::Focus,
            _ => return None, // unsupported pseudo-class -> fail closed
        };
        if sel.pseudo != Pseudo::None {
            return None;
        }
        sel.pseudo = pseudo;
        rest = format!("{}{}", &rest[..colon], &rest[colon + 1 + end..]);
    }

    let mut cur = String::new();
    let mut kind = 'e'; // 'e' = type/tag, '.' = class, '#' = id
    for ch in rest.chars() {
        if ch == '.' || ch == '#' {
            set_part(&mut sel, kind, &cur);
            cur.clear();
            kind = ch;
        } else {
            cur.push(ch);
        }
    }
    set_part(&mut sel, kind, &cur);
    if sel.type_name.is_none()
        && sel.id.is_none()
        && sel.classes.is_empty()
        && sel.attrs.is_empty()
        && sel.pseudo == Pseudo::None
    {
        None
    } else {
        Some(sel)
    }
}

/// Parse the inside of an attribute selector (the text between `[` and `]`).
/// `[href]`, `[href=x]`, `[href="x"]`, `[href^=x]`, `[href$=x]`, `[href*=x]`.
fn parse_attr(inner: &str) -> Option<AttrSelector> {
    let inner = inner.trim();
    if inner.is_empty() {
        return None;
    }
    // Find the operator, if any.
    let (name, op, value) = if let Some(idx) = inner.find("*=") {
        (
            inner[..idx].trim(),
            AttrOp::Contains,
            val_after(inner, idx + 2),
        )
    } else if let Some(idx) = inner.find("^=") {
        (
            inner[..idx].trim(),
            AttrOp::Prefix,
            val_after(inner, idx + 2),
        )
    } else if let Some(idx) = inner.find("$=") {
        (
            inner[..idx].trim(),
            AttrOp::Suffix,
            val_after(inner, idx + 2),
        )
    } else if let Some(idx) = inner.find('=') {
        (
            inner[..idx].trim(),
            AttrOp::Equals,
            val_after(inner, idx + 1),
        )
    } else {
        (inner, AttrOp::Exists, None)
    };
    if name.is_empty() {
        return None;
    }
    Some(AttrSelector {
        name: name.to_ascii_lowercase(),
        op,
        value: value.map(|v| v.to_string()),
    })
}

/// Extract the value part of an attribute selector after the operator,
/// stripping surrounding quotes if present.
fn val_after(s: &str, start: usize) -> Option<&str> {
    if start >= s.len() {
        return None;
    }
    let v = s[start..].trim();
    if v.is_empty() {
        return None;
    }
    let bytes = v.as_bytes();
    if (bytes[0] == b'"' || bytes[0] == b'\'') && v.len() >= 2 {
        Some(&v[1..v.len() - 1])
    } else {
        Some(v)
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
            .specificity_if_matches(&a, &[], InteractionState::default(), "div", None, &[])
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(&a, &[], InteractionState::default(), "span", None, &[])
            .is_none());
        assert!(rules[1]
            .specificity_if_matches(&a, &[], InteractionState::default(), "div", None, &["card"])
            .is_some());
        assert!(rules[1]
            .specificity_if_matches(&a, &[], InteractionState::default(), "div", None, &[])
            .is_none());
        assert!(rules[2]
            .specificity_if_matches(
                &a,
                &[],
                InteractionState::default(),
                "div",
                Some("hero"),
                &[]
            )
            .is_some());
    }

    #[test]
    fn compound_selector_requires_every_part() {
        let rules = parse_stylesheet("div.card#hero { color: red; }");
        assert_eq!(rules.len(), 1);
        let a = no_ancestors();
        assert!(rules[0]
            .specificity_if_matches(
                &a,
                &[],
                InteractionState::default(),
                "div",
                Some("hero"),
                &["card"]
            )
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(
                &a,
                &[],
                InteractionState::default(),
                "div",
                Some("hero"),
                &[]
            )
            .is_none());
        assert!(rules[0]
            .specificity_if_matches(
                &a,
                &[],
                InteractionState::default(),
                "span",
                Some("hero"),
                &["card"]
            )
            .is_none());
    }

    #[test]
    fn comma_separated_selector_list_matches_either() {
        let rules = parse_stylesheet("h1, h2 { color: red; }");
        assert_eq!(rules.len(), 1);
        let a = no_ancestors();
        assert!(rules[0]
            .specificity_if_matches(&a, &[], InteractionState::default(), "h1", None, &[])
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(&a, &[], InteractionState::default(), "h2", None, &[])
            .is_some());
        assert!(rules[0]
            .specificity_if_matches(&a, &[], InteractionState::default(), "h3", None, &[])
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
                .specificity_if_matches(
                    &ancestors,
                    &[],
                    InteractionState::default(),
                    "p",
                    None,
                    &[]
                )
                .is_some(),
            "div p should match a p descendant of a div"
        );
        // Without a div ancestor: no match.
        let no_ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        assert!(rules[0]
            .specificity_if_matches(
                &no_ancestors,
                &[],
                InteractionState::default(),
                "p",
                None,
                &[]
            )
            .is_none());
        // p without a div ancestor (say inside an article) doesn't match.
        let article: Vec<AncestorFrame<'_>> = vec![("article", None, empty)];
        assert!(rules[0]
            .specificity_if_matches(&article, &[], InteractionState::default(), "p", None, &[])
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
            .specificity_if_matches(&ancestors, &[], InteractionState::default(), "p", None, &[])
            .is_some());
    }

    #[test]
    fn three_step_chain_requires_matches_in_root_to_leaf_order() {
        // `html body p` matches a p inside a body inside an html.
        let rules = parse_stylesheet("html body p { color: red; }");
        let empty: &[&str] = &[];
        let good: Vec<AncestorFrame<'_>> = vec![("html", None, empty), ("body", None, empty)];
        assert!(rules[0]
            .specificity_if_matches(&good, &[], InteractionState::default(), "p", None, &[])
            .is_some());
        // Wrong order (body then html) shouldn't match.
        let reversed: Vec<AncestorFrame<'_>> = vec![("body", None, empty), ("html", None, empty)];
        assert!(rules[0]
            .specificity_if_matches(&reversed, &[], InteractionState::default(), "p", None, &[])
            .is_none());
    }

    #[test]
    fn descendant_chain_specificity_sums_the_compounds() {
        let rules = parse_stylesheet(".card p { color: red; }");
        let ancestors: Vec<AncestorFrame<'_>> = vec![("div", None, &["card"])];
        let spec = rules[0]
            .specificity_if_matches(&ancestors, &[], InteractionState::default(), "p", None, &[])
            .expect("should match");
        // (0, 1, 1): one class from `.card`, one type from `p`.
        assert_eq!(spec, (0, 1, 1));
    }

    #[test]
    fn child_combinator_now_parses() {
        // `>` (child combinator) is now supported — the rule parses.
        let rules = parse_stylesheet("div > p { color: red; }");
        assert_eq!(rules.len(), 1, "child combinator should parse now");
    }

    #[test]
    fn unsupported_combinator_syntax_still_fails_closed() {
        // `*` still unsupported — whole rule dropped. (`:hover`/`:focus` are
        // now supported; see pseudo-class tests.)
        assert_eq!(parse_stylesheet("* { color: red; }").len(), 0);
        // :active / :checked etc. still fail closed (unsupported pseudo-classes).
        assert_eq!(parse_stylesheet("a:active { color: red; }").len(), 0);
    }

    #[test]
    fn adjacent_sibling_combinator_parses() {
        // `a + b` (adjacent sibling) now parses.
        assert_eq!(parse_stylesheet("h1 + p { color: red; }").len(), 1);
    }

    #[test]
    fn general_sibling_combinator_parses() {
        // `a ~ b` (general sibling) now parses.
        assert_eq!(parse_stylesheet("h1 ~ p { color: red; }").len(), 1);
    }

    #[test]
    fn attribute_selector_parses_but_unknown_attr_fails_closed() {
        // `[data-x]` is a valid attribute selector and parses, but `data-x`
        // isn't an attribute the reducer exposes (only id/class), so it
        // matches nothing today — the rule still parses (1 rule) for future
        // attribute-plumbing.
        let rules = parse_stylesheet("[data-x] { color: red; }");
        assert_eq!(rules.len(), 1, "attribute selector should parse");
    }

    #[test]
    fn malformed_trailing_text_does_not_panic() {
        let rules = parse_stylesheet("div { color: red; } trailing garbage no brace");
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn attribute_selector_id_exists_matches() {
        // [id] matches an element with an id attribute.
        let chain = parse_selector("[id]").expect("[id] should parse");
        let ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "div",
            Some("main"),
            &[]
        )
        .is_some());
        // No id -> no match.
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "div",
            None,
            &[]
        )
        .is_none());
    }

    #[test]
    fn attribute_selector_class_equals_matches() {
        // NOTE: spaces inside a quoted attribute value (`[class="a b"]`)
        // aren't tokenized yet (the selector splitter is whitespace-based),
        // so this test uses a single-class value. Multi-word values are a
        // future tokenizer improvement.
        let chain = parse_selector(r#"[class="btn"]"#).expect("should parse");
        let ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        // Element with class "btn" matches.
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "a",
            None,
            &["btn"]
        )
        .is_some());
        // Different class -> no match.
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "a",
            None,
            &["nav"]
        )
        .is_none());
    }

    #[test]
    fn attribute_selector_class_prefix_matches() {
        let chain = parse_selector(r#"[class^="btn"]"#).expect("should parse");
        let ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "a",
            None,
            &["btn-primary"]
        )
        .is_some());
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "a",
            None,
            &["nav-link"]
        )
        .is_none());
    }

    #[test]
    fn attribute_selector_unknown_attr_fails_closed() {
        // [data-x] parses but data-x isn't exposed -> no match ever.
        let chain = parse_selector("[data-x]").expect("should parse");
        let ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "div",
            Some("a"),
            &["b"]
        )
        .is_none());
    }

    #[test]
    fn hover_matches_only_when_element_is_hovered() {
        let chain = parse_selector("#btn:hover").expect("should parse");
        let ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        // Not hovered -> no match.
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState {
                hovered_id: None,
                focused_id: None
            },
            "div",
            Some("btn"),
            &[]
        )
        .is_none());
        // Hovered -> match.
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState {
                hovered_id: Some("btn"),
                focused_id: None
            },
            "div",
            Some("btn"),
            &[]
        )
        .is_some());
        // A different element hovered -> no match.
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState {
                hovered_id: Some("other"),
                focused_id: None
            },
            "div",
            Some("btn"),
            &[]
        )
        .is_none());
    }

    #[test]
    fn focus_matches_only_when_element_is_focused() {
        let chain = parse_selector("input:focus").expect("should parse");
        let ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState {
                hovered_id: None,
                focused_id: None
            },
            "input",
            None,
            &[]
        )
        .is_none());
        // :focus needs an id to identify the focused element; an id-less
        // input can't be the focused one (no stable identity to match on).
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState {
                hovered_id: None,
                focused_id: Some("q")
            },
            "input",
            None,
            &[]
        )
        .is_none());
        // input#q focused -> match.
        let chain2 = parse_selector("#q:focus").expect("should parse");
        assert!(match_chain(
            &chain2,
            &ancestors,
            &[],
            InteractionState {
                hovered_id: None,
                focused_id: Some("q")
            },
            "input",
            Some("q"),
            &[]
        )
        .is_some());
    }

    #[test]
    fn unsupported_pseudo_class_fails_closed() {
        // :active, :nth-child, etc. still fail closed.
        assert!(parse_selector("a:active").is_none());
        assert!(parse_selector("li:nth-child(2)").is_none());
        assert!(parse_selector("a:checked").is_none());
    }

    #[test]
    fn compound_with_attribute_and_class() {
        // div.card[id] -- a div with class card and an id.
        let chain = parse_selector("div.card[id]").expect("should parse");
        let ancestors: Vec<AncestorFrame<'_>> = Vec::new();
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "div",
            Some("x"),
            &["card"]
        )
        .is_some());
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "div",
            None,
            &["card"]
        )
        .is_none());
    }

    #[test]
    fn child_combinator_in_three_step_chain() {
        // div > section p: p must be a descendant of a section that is a
        // direct child of a div.
        let chain = parse_selector("div > section p").expect("should parse");
        let empty: &[&str] = &[];
        // ancestors: [div, section] -> p descendant of section, section child of div.
        let ancestors: Vec<AncestorFrame<'_>> =
            vec![("div", None, empty), ("section", None, empty)];
        assert!(match_chain(
            &chain,
            &ancestors,
            &[],
            InteractionState::default(),
            "p",
            None,
            &[]
        )
        .is_some());
        // ancestors: [div, article, section] -> section is NOT a direct child
        // of div (article intervenes) -> no match.
        let ancestors2: Vec<AncestorFrame<'_>> = vec![
            ("div", None, empty),
            ("article", None, empty),
            ("section", None, empty),
        ];
        assert!(match_chain(
            &chain,
            &ancestors2,
            &[],
            InteractionState::default(),
            "p",
            None,
            &[]
        )
        .is_none());
    }
}
