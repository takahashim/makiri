//! The internal DTD subset: checked in full, applied never.
//!
//! §5.1 requires even a non-validating processor to check the internal subset
//! for well-formedness, so it is parsed rather than skipped. Makiri does not
//! APPLY what it declares, so a declaration that WOULD change the tree - an
//! attribute default or a non-CDATA attribute type (§3.3.2-3.3.3), or a
//! parameter-entity reference whose replacement text could carry either - makes
//! the parse fail with [`Status::Unsupported`] instead of being silently
//! ignored. Entity declarations are accepted: declaring one changes nothing
//! until a reference to it, and that reference is refused where it occurs.
//!
//! Nothing here builds a node. That is why it holds a [`Cursor`] and a
//! [`Declared`] rather than a parser: a validator that keeps nothing has no
//! business reaching the document, the namespace scope or the node arena, and
//! with these types it cannot.

#![forbid(unsafe_code)]

use super::cursor::{find, Cursor, InSlice, R};
use crate::falloc::Reserve;
use crate::xml::chars::is_reserved_pi_target;
use crate::xml::chars::validate_name;
use crate::xml::{Status, MAX_DEPTH};

/// An ExternalID's identifiers (§4.2.2). Either may be absent, and which one is
/// present is the difference between SYSTEM and PUBLIC, so they are named
/// rather than positional.
#[derive(Clone, Copy, Default)]
pub(super) struct ExternalId {
    pub public: Option<InSlice>,
    pub system: Option<InSlice>,
}

/* ---- the two productions both the DOCTYPE parser and the subset need ---- */

/// ExternalID (§4.2.2), or with `public_only_ok` also NOTATION's PublicID:
/// 'SYSTEM' S SystemLiteral | 'PUBLIC' S PubidLiteral (S SystemLiteral)?.
pub(super) fn scan_external_id(cur: &mut Cursor<'_>, public_only_ok: bool) -> R<ExternalId> {
    if cur.eat_keyword(b"SYSTEM") {
        cur.require_space()?;
        return Ok(ExternalId {
            public: None,
            system: Some(cur.scan_char_literal()?),
        });
    }
    if !cur.eat_keyword(b"PUBLIC") {
        return cur.syntax();
    }
    cur.require_space()?;
    let public = Some(cur.scan_pubid_literal()?);
    let had_space = cur.skip_some_ws();
    if had_space && matches!(cur.peek(), Some(b'"' | b'\'')) {
        return Ok(ExternalId {
            public,
            system: Some(cur.scan_char_literal()?),
        });
    }
    if public_only_ok {
        return Ok(ExternalId {
            public,
            system: None,
        });
    }
    cur.syntax()
}

/// An EntityValue / AttValue literal: Chars, with every '&' a well-formed
/// Reference. An AttValue may not hold '<'; an EntityValue may not hold
/// '%', since in the internal subset a parameter-entity reference may not
/// occur inside a declaration (WFC: PEs in Internal Subset). In an AttValue
/// '%' is an ordinary character.
pub(super) fn scan_ref_literal(cur: &mut Cursor<'_>, att_value: bool) -> R {
    let s = cur.scan_char_literal()?;
    let v = cur.slice(s);
    let mut i = 0;
    while i < v.len() {
        match v[i] {
            b'%' if !att_value => return cur.syntax(),
            b'<' if att_value => return cur.syntax(),
            b'&' => {
                let Some(end) = find(&v[i..], b';') else {
                    return cur.syntax();
                };
                let body = &v[i + 1..i + end];
                let ok = match body.strip_prefix(b"#") {
                    Some(num) => match num.strip_prefix(b"x") {
                        Some(hex) => !hex.is_empty() && hex.iter().all(u8::is_ascii_hexdigit),
                        None => !num.is_empty() && num.iter().all(u8::is_ascii_digit),
                    },
                    None => validate_name(body),
                };
                if !ok {
                    return cur.syntax();
                }
                i += end;
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// What a DOCTYPE declared that the REST of the parse still has to know: which
/// general entities exist, so a later reference to one reports as unexpanded
/// rather than as undeclared.
#[derive(Default)]
pub(super) struct Declared {
    /// The general entities the internal subset declares, as input slices.
    names: Vec<InSlice>,
    /// The DOCTYPE names an external subset, which a non-validating processor
    /// does not read; an entity it declares is equally not expanded.
    external_subset: bool,
}

impl Declared {
    pub(super) fn note_external_subset(&mut self, yes: bool) {
        self.external_subset = yes;
    }

    /// Whether `s` references a general entity that a DTD declares (or may
    /// declare, in an external subset) - one Makiri does not expand, as opposed
    /// to an undeclared name, which is a well-formedness error.
    ///
    /// Takes the cursor rather than the raw input: a declared name is an
    /// [`InSlice`], and `Cursor::slice` is how every other one is read. Handing
    /// the whole input out instead was the only reason `Cursor::input` existed.
    pub(super) fn refs_unexpanded_entity(&self, cur: &Cursor<'_>, s: &[u8]) -> bool {
        let mut i = 0;
        while let Some(at) = find(&s[i..], b'&') {
            i += at + 1;
            if s.get(i) == Some(&b'#') {
                continue;
            }
            let Some(end) = find(&s[i..], b';') else {
                return false;
            };
            let name = &s[i..i + end];
            if !matches!(name, b"lt" | b"gt" | b"amp" | b"apos" | b"quot")
                && (self.external_subset || self.names.iter().any(|&n| cur.slice(n) == name))
            {
                return true;
            }
            i += end;
        }
        false
    }
}

/// The validator, for the length of one `intSubset`.
pub(super) struct Subset<'c, 'a> {
    cur: &'c mut Cursor<'a>,
    declared: &'c mut Declared,
}

impl<'c, 'a> Subset<'c, 'a> {
    pub(super) fn new(cur: &'c mut Cursor<'a>, declared: &'c mut Declared) -> Self {
        Subset { cur, declared }
    }

    /// `intSubset` up to (not past) its closing ']'. Answers whether it holds a
    /// declaration Makiri refuses to ignore. The whole subset is checked first,
    /// so a malformed one reports as malformed.
    pub(super) fn parse(&mut self) -> R<bool> {
        let mut unsupported = false;
        loop {
            self.cur.skip_ws();
            match self.cur.peek() {
                None => return self.cur.syntax(),
                Some(b']') => return Ok(unsupported),
                Some(b'%') => {
                    /* DeclSep: a PEReference. */
                    self.cur.advance();
                    self.cur.scan_ncname()?;
                    if self.cur.peek() != Some(b';') {
                        return self.cur.syntax();
                    }
                    self.cur.advance();
                    unsupported = true;
                }
                Some(b'<') => {
                    if self.cur.starts(b"<!--") {
                        self.cur.advance_n(4);
                        let body = self.cur.scan_until_close(b"-->", Some(b'-'))?;
                        self.cur.take_close(body, b"-->");
                    } else if self.cur.starts(b"<?") {
                        self.cur.advance_n(2);
                        self.subset_pi()?;
                    } else if self.eat_decl(b"<!ELEMENT")? {
                        self.element_decl()?;
                    } else if self.eat_decl(b"<!ATTLIST")? {
                        unsupported |= self.attlist_decl()?;
                    } else if self.eat_decl(b"<!ENTITY")? {
                        self.entity_decl()?;
                    } else if self.eat_decl(b"<!NOTATION")? {
                        self.notation_decl()?;
                    } else {
                        return self.cur.syntax();
                    }
                }
                Some(_) => return self.cur.syntax(),
            }
        }
    }

    /// A declaration keyword, which must be followed by white space.
    fn eat_decl(&mut self, kw: &[u8]) -> R<bool> {
        if !self.cur.starts(kw) {
            return Ok(false);
        }
        self.cur.advance_n(kw.len());
        self.cur.require_space()?;
        Ok(true)
    }

    /// White space, then the declaration's closing '>'.
    fn end_decl(&mut self) -> R {
        self.cur.skip_ws();
        if self.cur.peek() != Some(b'>') {
            return self.cur.syntax();
        }
        self.cur.advance();
        Ok(())
    }

    /// '<?' PI '?>' in the subset; nothing is kept.
    fn subset_pi(&mut self) -> R {
        let t = self.cur.scan_ncname()?;
        if is_reserved_pi_target(self.cur.slice(t)) {
            return self.cur.syntax();
        }
        if !self.cur.starts(b"?>") {
            self.cur.need_space()?;
        }
        let body = self.cur.scan_until_close(b"?>", None)?;
        self.cur.take_close(body, b"?>");
        Ok(())
    }

    /// elementdecl (§3.2), after '<!ELEMENT' S.
    fn element_decl(&mut self) -> R {
        self.cur.scan_qname()?;
        self.cur.require_space()?;
        if !(self.cur.eat_keyword(b"EMPTY") || self.cur.eat_keyword(b"ANY")) {
            if self.cur.peek() != Some(b'(') {
                return self.cur.syntax();
            }
            self.cur.advance();
            self.cur.skip_ws();
            if self.cur.eat_keyword(b"#PCDATA") {
                self.mixed()?;
            } else {
                self.cp_group(0)?;
                self.eat_quantifier();
            }
        }
        self.end_decl()
    }

    /// Mixed (§3.2.2), after '(' S? '#PCDATA'.
    fn mixed(&mut self) -> R {
        let mut names = false;
        loop {
            self.cur.skip_ws();
            match self.cur.peek() {
                Some(b')') => {
                    self.cur.advance();
                    /* with names the '*' is required; bare #PCDATA may take one */
                    if self.cur.peek() == Some(b'*') {
                        self.cur.advance();
                    } else if names {
                        return self.cur.syntax();
                    }
                    return Ok(());
                }
                Some(b'|') => {
                    self.cur.advance();
                    self.cur.skip_ws();
                    self.cur.scan_qname()?;
                    names = true;
                }
                _ => return self.cur.syntax(),
            }
        }
    }

    /// A choice or seq (§3.2.1) after its '(' - one kind of separator
    /// throughout. Nesting is bounded like element nesting, so a hostile
    /// content model cannot exhaust the stack.
    fn cp_group(&mut self, depth: usize) -> R {
        if depth >= MAX_DEPTH {
            return self.cur.limit();
        }
        let mut sep: Option<u8> = None;
        loop {
            self.cur.skip_ws();
            if self.cur.peek() == Some(b'(') {
                self.cur.advance();
                self.cp_group(depth + 1)?;
            } else {
                self.cur.scan_qname()?;
            }
            self.eat_quantifier();
            self.cur.skip_ws();
            match self.cur.peek() {
                Some(b')') => {
                    self.cur.advance();
                    return Ok(());
                }
                Some(c @ (b'|' | b',')) if sep.is_none_or(|s| s == c) => {
                    sep = Some(c);
                    self.cur.advance();
                }
                _ => return self.cur.syntax(),
            }
        }
    }

    fn eat_quantifier(&mut self) {
        if matches!(self.cur.peek(), Some(b'?' | b'*' | b'+')) {
            self.cur.advance();
        }
    }

    /// AttlistDecl (§3.3), after '<!ATTLIST' S. Answers whether any AttDef is
    /// one Makiri refuses: a non-CDATA type, or a default value.
    fn attlist_decl(&mut self) -> R<bool> {
        self.cur.scan_qname()?;
        let mut unsupported = false;
        loop {
            let had_space = self.cur.skip_some_ws();
            if self.cur.peek() == Some(b'>') {
                self.cur.advance();
                return Ok(unsupported);
            }
            if !had_space {
                return self.cur.syntax(); /* AttDef ::= S Name ... */
            }
            self.cur.scan_qname()?;
            self.cur.require_space()?;
            let cdata = self.att_type()?;
            self.cur.require_space()?;
            let defaulted =
                if self.cur.eat_keyword(b"#REQUIRED") || self.cur.eat_keyword(b"#IMPLIED") {
                    false
                } else {
                    if self.cur.eat_keyword(b"#FIXED") {
                        self.cur.require_space()?;
                    }
                    scan_ref_literal(self.cur, true)?;
                    true
                };
            unsupported |= !cdata || defaulted;
        }
    }

    /// AttType (§3.3.1); answers whether it is CDATA.
    fn att_type(&mut self) -> R<bool> {
        if self.cur.eat_keyword(b"CDATA") {
            return Ok(true);
        }
        /* longest first, so IDREFS is not read as ID + "REFS" */
        for kw in [
            &b"IDREFS"[..],
            b"IDREF",
            b"ID",
            b"ENTITIES",
            b"ENTITY",
            b"NMTOKENS",
            b"NMTOKEN",
        ] {
            if self.cur.eat_keyword(kw) {
                return Ok(false);
            }
        }
        let notation = self.cur.eat_keyword(b"NOTATION");
        if notation {
            self.cur.require_space()?;
        }
        if self.cur.peek() != Some(b'(') {
            return self.cur.syntax();
        }
        self.cur.advance();
        loop {
            self.cur.skip_ws();
            if notation {
                self.cur.scan_ncname()?;
            } else {
                self.cur.scan_nmtoken()?;
            }
            self.cur.skip_ws();
            match self.cur.peek() {
                Some(b')') => {
                    self.cur.advance();
                    return Ok(false);
                }
                Some(b'|') => self.cur.advance(),
                _ => return self.cur.syntax(),
            }
        }
    }

    /// EntityDecl (§4.2), after '<!ENTITY' S. A general entity's name is
    /// recorded, so a reference to it reports as unexpanded, not undeclared.
    fn entity_decl(&mut self) -> R {
        let pe = self.cur.peek() == Some(b'%');
        if pe {
            self.cur.advance();
            self.cur.require_space()?;
        }
        let name = self.cur.scan_ncname()?;
        self.cur.require_space()?;
        if matches!(self.cur.peek(), Some(b'"' | b'\'')) {
            scan_ref_literal(self.cur, false)?;
        } else {
            scan_external_id(self.cur, false)?;
            if !pe {
                /* NDataDecl ::= S 'NDATA' S Name */
                if self.cur.skip_some_ws() && self.cur.eat_keyword(b"NDATA") {
                    self.cur.require_space()?;
                    self.cur.scan_ncname()?;
                }
            }
        }
        self.end_decl()?;
        if !pe {
            if self.declared.names.falloc_reserve(1).is_err() {
                return self.cur.fail(Status::Oom);
            }
            self.declared.names.push(name);
        }
        Ok(())
    }

    /// NotationDecl (§4.7), after '<!NOTATION' S.
    fn notation_decl(&mut self) -> R {
        self.cur.scan_ncname()?;
        self.cur.require_space()?;
        scan_external_id(self.cur, true)?;
        self.end_decl()
    }
}
