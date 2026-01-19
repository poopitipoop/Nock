use std::cell::Cell;
use std::cmp;
use std::cmp::Ordering;
use std::collections::{HashMap, *};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufWriter, Write};
use std::ops::BitAnd;
use std::path::PathBuf;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use bitvec::order::Lsb0;
use bitvec::prelude::*;
use bitvec::slice::BitSlice;
use bitvec::vec::BitVec;
use bytes::Bytes;
use chumsky::input::{Input, MapExtra, StrInput, Stream, ValueInput};
use chumsky::prelude::*;
use chumsky::span::Span;
use either::Either::{Left, Right};
use ibig::UBig;
use nockapp::noun::slab::{slab_mug, slab_noun_equality, NounSlab};
use nockapp::AtomExt;
use nockvm::jets::util::slot;
use nockvm::noun::{Atom, DirectAtom, Noun, D, DIRECT_MAX, NO, T, YES};
use nockvm_macros::tas;
use num_bigint::BigUint;
use num_traits::identities::Zero;
use num_traits::{FromPrimitive, Num, One, ToPrimitive};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ast::hoon::*;
pub type Err<'src> = extra::Full<Rich<'src, char>, (), ()>;

pub trait ParserExt<'src, O>: Parser<'src, &'src str, O, Err<'src>> + Clone + 'src {}

impl<'src, O, P> ParserExt<'src, O> for P where
    P: Parser<'src, &'src str, O, Err<'src>> + Clone + 'src
{
}

//
// String -> ParsedAtom conversion functions
//

pub fn string_to_atom(s: String) -> ParsedAtom {
    let vec_u128: Vec<u128> = s.chars().map(|c| c as u128).collect();

    rap(3, &vec_u128)
}

pub fn ta_to_atom(s: String) -> ParsedAtom {
    if s == "~.".to_string() {
        return ParsedAtom::Small(0);
    }
    let vec_u128: Vec<u128> = s.chars().map(|c| c as u128).collect();

    rap(3, &vec_u128)
}

pub fn term_to_atom(s: String) -> ParsedAtom {
    if s == "$".to_string() {
        return ParsedAtom::Small(0);
    }
    let vec_u128: Vec<u128> = s.chars().map(|c| c as u128).collect();

    rap(3, &vec_u128)
}

//  @ud to @
pub fn decimal_to_atom(s: String) -> ParsedAtom {
    ParsedAtom::Small(s.parse::<u128>().expect("decimal_to_atom failed"))
}

//  @ux to @
pub fn hex_to_atom(s: String) -> ParsedAtom {
    let clean = s.strip_prefix("0x").unwrap_or(&s);

    if clean.len() <= 32 {
        if let Ok(n) = u128::from_str_radix(clean, 16) {
            return ParsedAtom::Small(n);
        }
    }

    let big = BigUint::parse_bytes(clean.as_bytes(), 16).expect("invalid hex in big atom");

    ParsedAtom::Big(big)
}

//  @ub to @
pub fn binary_to_atom(s: String) -> ParsedAtom {
    ParsedAtom::Small(u128::from_str_radix(&s, 2).expect("binary_to_atom failed"))
}

//  @t to @
pub fn cord_chars_to_atom(chars: Vec<char>) -> ParsedAtom {
    let mut atom = BigUint::zero();
    let mut power = BigUint::from(1u32);
    let base = BigUint::from(256u32);

    for &c in &chars {
        let byte = BigUint::from(c as u32 & 0xFF);
        atom += &byte * &power;
        power *= &base;
    }

    ParsedAtom::Big(atom)
}

const ALPH64: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-~";

//  @uw to @
pub fn base64_to_atom(s: String) -> ParsedAtom {
    let mut n: u128 = 0;

    for ch in s.chars() {
        let v = match ALPH64.find(ch) {
            Some(i) => i as u128,
            None => panic!("invalid digit '{ch}' in base64"),
        };

        n = n.checked_mul(64).expect("value exceeds u128 range (mul)");

        n = n.checked_add(v).expect("value exceeds u128 range (add)");
    }

    ParsedAtom::Small(n)
}

const ALPH32: &str = "0123456789abcdefghijklmnopqrstuv";

//  @uv to @
pub fn base32_to_atom(s: String) -> ParsedAtom {
    let mut n: u128 = 0;

    for ch in s.chars() {
        let v = match ALPH32.find(ch) {
            Some(i) => i as u128,
            None => panic!("invalid digit '{ch}' in base32"),
        };

        n = n.checked_mul(32).expect("value exceeds u128 range (mul)");

        n = n.checked_add(v).expect("value exceeds u128 range (add)");
    }

    ParsedAtom::Small(n)
}

// +fim
pub fn base58_to_atom(s: String) -> Option<ParsedAtom> {
    let yek = build_yek();

    let digits: Vec<u8> = s
        .chars()
        .map(|ch| cha_fa(&yek, ch))
        .collect::<Option<_>>()?;

    let a = ParsedAtom::Big(bass_58(&digits));
    den_fa(&a)
}

pub fn ipv4_to_atom(s: String) -> Option<ParsedAtom> {
    let addr = s.parse::<std::net::Ipv4Addr>().ok()?;

    let ip_num = u32::from_be_bytes(addr.octets());

    Some(ParsedAtom::Small(ip_num.into()))
}

pub fn ipv6_to_atom(s: String) -> Option<ParsedAtom> {
    let addr = s.parse::<std::net::Ipv6Addr>().ok()?;
    let num = u128::from_be_bytes(addr.octets());
    Some(ParsedAtom::Small(num))
}

pub fn basal(bas: BaseType) -> Hoon {
    match bas {
        BaseType::Atom(a) => {
            let literal = if a == "da" {
                ParsedAtom::Small(year(true, 2000, 1, 1, 0, 0, 0, &Vec::new()))
            } else {
                decimal_to_atom("0".to_string())
            };
            Hoon::Sand(a, NounExpr::ParsedAtom(literal))
        }
        BaseType::NounExpr => {
            let rock0 = Box::new(Hoon::Rock(
                "$".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let rock1 = Box::new(Hoon::Rock(
                "$".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(1)),
            ));
            let rock0_clone = rock0.clone();
            let rock0_clone2 = rock0.clone();
            Hoon::KetLus(
                Box::new(Hoon::DotTar(
                    rock0,
                    Box::new(Hoon::Pair(rock0_clone, rock1)),
                )),
                rock0_clone2,
            )
        }
        BaseType::Cell => {
            let noun = Box::new(basal(BaseType::NounExpr));
            let noun_clone = noun.clone();
            Hoon::Pair(noun, noun_clone)
        }
        BaseType::Flag => {
            let rock0 = Box::new(Hoon::Rock(
                "$".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let rock0_clone = rock0.clone();
            let rock1_clone = rock0.clone();
            Hoon::KetLus(Box::new(Hoon::DotTis(rock0, rock0_clone)), rock1_clone)
        }
        BaseType::Null => Hoon::Rock("$".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
        BaseType::Void => Hoon::ZapZap,
    }
}

pub fn function(
    fun: Spec,
    arg: Spec,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    Hoon::TisGar(
        Box::new(Hoon::Pair(
            Box::new(example(&fun.clone(), dom, hay, cox, &vec![], &None, &None)),
            Box::new(example(&arg.clone(), dom, hay, cox, &vec![], &None, &None)),
        )),
        Box::new(Hoon::KetBar(Box::new(Hoon::BarCol(
            Box::new(Hoon::Axis(2)),
            Box::new(Hoon::Axis(15)),
        )))),
    )
}

pub fn interface(
    variance: Vair,
    payload: Spec,
    arms: HashMap<String, Spec>,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    let map: HashMap<String, Hoon> = arms
        .into_iter()
        .map(|(term, spec)| (term, example(&spec, dom, hay, cox, &vec![], &None, &None)))
        .collect();
    let brcn = Hoon::BarCen(None, HashMap::from([("$".to_string(), (None, map))]));

    let example_res = example(&payload, dom, hay, cox, &vec![], &None, &None);
    let tsgr = Hoon::TisGar(Box::new(example_res), Box::new(brcn));
    match variance {
        Vair::Gold => tsgr,
        Vair::Lead => Hoon::KetWut(Box::new(tsgr)),
        Vair::Zinc => Hoon::KetPam(Box::new(tsgr)),
        Vair::Iron => Hoon::KetBar(Box::new(tsgr)),
    }
}

// TODO: accept args by ref?
pub fn spore(
    spec: Spec,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    let subject = match def {
        Some(d) => d,
        None => spore_recursion(spec, dom, hay, cox, bug, nut, def),
    };
    let ketlus_tail = home(subject, Vec::new(), dom);
    Hoon::KetLus(
        Box::new(Hoon::Bust(BaseType::NounExpr)),
        Box::new(ketlus_tail),
    )
}

pub fn spore_recursion(
    spec: Spec,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    match spec {
        Spec::Base(b) => match b {
            BaseType::Void => {
                Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)))
            }
            _ => basal(b),
        },
        Spec::BucBuc(s, map) => {
            let mut new_cox = cox;
            new_cox.extend(map);
            new_cox.insert("$".to_string(), *s.clone());
            spore_recursion(*s, dom, hay, new_cox, bug, nut, def)
        }
        Spec::Dbug(spot, spec) => {
            let tail = spore_recursion(*spec, dom, hay, cox, bug, nut, def);
            Hoon::Dbug(spot, Box::new(tail))
        }
        Spec::Leaf(term, atom) => Hoon::Rock(term, NounExpr::ParsedAtom(atom)),
        Spec::Loop(term) => {
            let spec = cox.get(&term).expect("Spec-Loop: Name not found");
            spore_recursion(spec.clone(), dom, hay, cox, bug, nut, def)
        }
        Spec::Like(wing, wings) => {
            let p = unreel(wing, wings);
            spore_recursion(Spec::BucMic(p), dom, hay, cox, bug, nut, def)
        }
        Spec::Made(_, q) => spore_recursion(*q, dom, hay, cox, bug, nut, def),
        Spec::Make(hoon, specs) => {
            let p = unfold(hoon, specs);
            spore_recursion(Spec::BucMic(p), dom, hay, cox, bug, nut, def)
        }
        Spec::Name(term, spec) => spore_recursion(*spec, dom, hay, cox, bug, nut, def),
        Spec::Over(wing, spec) => spore_recursion(*spec, dom, wing, cox, bug, nut, def),
        Spec::BucBar(spec, hoon) => spore_recursion(*spec, dom, hay, cox, bug, nut, def),
        Spec::BucCab(_) => Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
        Spec::BucCol(spec, specs) => {
            spore_buccol_recursion(*spec, specs, dom, hay, cox, bug, nut, def)
        }
        Spec::BucCen(spec, specs) => {
            spore_buccen_recursion(*spec, specs, dom, hay, cox, bug, nut, def)
        }
        Spec::BucHep(spec, specs) => {
            Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)))
        }
        Spec::BucGal(p_spec, q_spec) => spore_recursion(*q_spec, dom, hay, cox, bug, nut, def),
        Spec::BucGar(p_spec, q_spec) => spore_recursion(*q_spec, dom, hay, cox, bug, nut, def),
        Spec::BucKet(p_spec, q_spec) => spore_recursion(*q_spec, dom, hay, cox, bug, nut, def),
        Spec::BucLus(stud, spec) => {
            let tail = spore_recursion(*spec, dom, hay, cox, bug, nut, def);
            Hoon::Note(Note::Know(stud), Box::new(tail))
        }
        Spec::BucMic(hoon) => Hoon::TisGal(Box::new(Hoon::Axis(6)), Box::new(hoon)),
        Spec::BucPam(spec, hoon) => spore_recursion(*spec, dom, hay, cox, bug, nut, def),
        Spec::BucSig(hoon, spec) => Hoon::KetHep(spec, Box::new(hoon)),
        Spec::BucTis(skin, spec) => {
            let tail = spore_recursion(*spec, dom, hay, cox, bug, nut, def);
            Hoon::KetTis(skin, Box::new(tail))
        }
        Spec::BucPat(p_spec, q_spec) => spore_recursion(*p_spec, dom, hay, cox, bug, nut, def),
        Spec::BucWut(spec, specs) => {
            spore_bucwut_recursion(*spec, specs, dom, hay, cox, bug, nut, def)
        }
        Spec::BucDot(..) | Spec::BucFas(..) | Spec::BucTic(..) | Spec::BucZap(..) => {
            Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)))
        }
    }
}

pub fn spore_buccol_recursion(
    spec: Spec,
    list_spec: Vec<Spec>,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    if list_spec.is_empty() {
        spore_recursion(spec, dom, hay, cox, bug, nut, def)
    } else {
        let head = spore_recursion(
            spec,
            dom.clone(),
            hay.clone(),
            cox.clone(),
            bug.clone(),
            nut.clone(),
            def.clone(),
        );
        let tail = spore_buccol_recursion(
            list_spec.first().unwrap().clone(),
            list_spec[1..].to_vec(),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        );
        Hoon::Pair(Box::new(head), Box::new(tail))
    }
}

pub fn spore_bucwut_recursion(
    spec: Spec,
    list_spec: Vec<Spec>,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    if list_spec.is_empty() {
        spore_recursion(spec, dom, hay, cox, bug, nut, def)
    } else {
        spore_bucwut_recursion(
            list_spec.first().unwrap().clone(),
            list_spec[1..].to_vec(),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        )
    }
}

pub fn spore_buccen_recursion(
    spec: Spec,
    list_spec: Vec<Spec>,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    if list_spec.is_empty() {
        spore_recursion(spec, dom, hay, cox, bug, nut, def)
    } else {
        spore_buccen_recursion(
            list_spec.first().unwrap().clone(),
            list_spec[1..].to_vec(),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        )
    }
}

pub fn example(
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    match mod_ {
        Spec::Base(b) => decorate(basal(b.clone()), bug.clone(), nut.clone()),
        Spec::Dbug(spot, inner) => {
            let mut bug = bug.clone();
            bug.push(spot.clone());
            example(&inner, dom, hay, cox, &bug, nut, def)
        }
        Spec::Leaf(term, atom) => decorate(
            Hoon::Rock(term.clone(), NounExpr::ParsedAtom(atom.clone())),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Like(wing, list) => example(
            &Spec::BucMic(unreel(wing.clone(), list.clone())),
            dom,
            wing,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::Loop(term) => Hoon::Limb(term.clone()),
        Spec::Made((t, list), inner) => {
            let pieces = list
                .iter()
                .map(|s| vec![Limb::Term(s.to_string())])
                .collect();
            example(
                &inner,
                dom,
                hay,
                cox,
                bug,
                &Some(Note::Made(t.to_string(), Some(pieces))),
                def,
            )
        }
        Spec::Make(head, tail) => example(
            &Spec::BucMic(unfold(head.clone(), tail.clone())),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::Name(term, inner) => example(
            &inner,
            dom,
            hay,
            cox,
            bug,
            &Some(Note::Made(term.to_string(), None)),
            def,
        ),
        Spec::Over(wing, inner) => example(&inner, dom, wing, cox, bug, nut, def),
        Spec::BucCab(p) => decorate(
            home(p.clone(), hay.clone(), dom.clone()),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucCol(head, tail) => {
            let mut result = example(head, dom, hay, cox, &vec![], &None, &None);

            for x in tail.iter().rev() {
                let next = example(&x, dom, hay, cox, &vec![], &None, &None);
                result = Hoon::Pair(Box::new(next), Box::new(result));
            }

            decorate(result, bug.clone(), nut.clone())
        }
        Spec::BucHep(p, q) => {
            let function_res = function(
                *p.clone(),
                *q.clone(),
                mod_,
                dom,
                hay,
                cox,
                &vec![],
                &None,
                &None,
            );
            decorate(function_res, bug.clone(), nut.clone())
        }
        Spec::BucMic(inner) => {
            let tsgl = Hoon::TisGal(
                Box::new(Hoon::Limb("$".to_string())),
                Box::new(inner.clone()),
            );
            decorate(
                home(tsgl, hay.clone(), dom.clone()),
                bug.clone(),
                nut.clone(),
            )
        }
        Spec::BucSig(inner, list) => Hoon::KetLus(
            Box::new(example(&list, dom, hay, cox, bug, nut, def)),
            Box::new(home(inner.clone(), hay.clone(), dom.clone())),
        ),
        Spec::BucLus(stud, inner) => decorate(
            Hoon::Note(
                Note::Know(stud.clone()),
                Box::new(example(&inner.clone(), dom, hay, cox, bug, nut, def)),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucTis(skin, inner) => decorate(
            Hoon::KetTis(
                skin.clone(),
                Box::new(example(&inner.clone(), dom, hay, cox, bug, nut, def)),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucDot(inner, map) => vair_case(
            Vair::Gold,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::BucFas(inner, map) => vair_case(
            Vair::Iron,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::BucZap(inner, map) => vair_case(
            Vair::Lead,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::BucTic(inner, map) => vair_case(
            Vair::Zinc,
            *inner.clone(),
            map.clone(),
            mod_,
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        _ => {
            let spore_result = spore(
                mod_.clone(),
                dom.clone(),
                hay.clone(),
                cox.clone(),
                bug.clone(),
                nut.clone(),
                def.clone(),
            );
            let dom = peg(dom, 3).expect("example +peg failed");
            let relative_result = relative(2, mod_, dom, hay, cox, bug, nut, def);
            Hoon::TisLus(Box::new(spore_result), Box::new(relative_result))
        }
    }
}

// used in +example
fn vair_case(
    vair: Vair,
    payload: Spec,
    arms: HashMap<String, Spec>,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    let hoon = interface(vair, payload, arms, mod_, dom, hay, cox, bug, nut, def);
    decorate(
        home(hoon, hay.clone(), dom.clone()),
        bug.clone(),
        nut.clone(),
    )
}

pub fn basic(
    bas: BaseType,
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    match bas {
        BaseType::Atom(a) => {
            let cnls = Hoon::CenLus(
                Box::new(Hoon::Limb("ruth".to_string())),
                Box::new(Hoon::Sand(
                    "ta".to_string(),
                    NounExpr::ParsedAtom(string_to_atom(a)),
                )),
                Box::new(Hoon::Axis(axe)),
            );

            let example_res = Box::new(example(mod_, dom, hay, cox, bug, nut, def));

            let wtpt_limb = Limb::Axis(axe);
            let wtpt_wing: Vec<Limb> = vec![wtpt_limb];
            let wtpt = Hoon::WutPat(wtpt_wing, Box::new(Hoon::Axis(axe)), Box::new(Hoon::ZapZap));

            let zppt_limb = Limb::Parent(0, Some("ruth".to_string()));
            let zppt_wing: Vec<Limb> = vec![zppt_limb];
            let zppt_list_wing: Vec<Vec<Limb>> = vec![zppt_wing];
            let zppt = Hoon::ZapPat(zppt_list_wing, Box::new(cnls), Box::new(wtpt));

            Hoon::KetLus(example_res, Box::new(zppt))
        }
        BaseType::Cell => {
            let example_res = Box::new(example(mod_, dom, hay, cox, bug, nut, def));
            let wing = Limb::Axis(axe);
            let wing: Vec<Limb> = vec![wing];
            let mut p = wing.clone();
            p.insert(0, Limb::Axis(2));
            let mut q = wing.clone();
            q.insert(0, Limb::Axis(3));
            let pair = Hoon::Pair(Box::new(Hoon::Wing(p)), Box::new(Hoon::Wing(q)));

            Hoon::KetLus(example_res, Box::new(pair))
        }
        BaseType::Flag => {
            let rock = Box::new(Hoon::Rock(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let dtts = Box::new(Hoon::DotTis(
                Box::new(Hoon::Rock(
                    "$".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                )),
                Box::new(Hoon::Axis(axe)),
            ));
            let wtgr = Box::new(Hoon::WutGar(
                Box::new(Hoon::DotTis(
                    Box::new(Hoon::Rock(
                        "$".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                    )),
                    Box::new(Hoon::Axis(axe)),
                )),
                Box::new(Hoon::Rock(
                    "f".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                )),
            ));
            Hoon::WutCol(dtts, rock, wtgr)
        }
        BaseType::Null => {
            let rock = Box::new(Hoon::Rock(
                "n".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            ));
            let dtts = Box::new(Hoon::DotTis(
                Box::new(Hoon::Bust(BaseType::NounExpr)),
                Box::new(Hoon::Axis(axe)),
            ));
            Hoon::WutGar(dtts, rock)
        }
        BaseType::NounExpr => Hoon::Axis(axe),
        BaseType::Void => Hoon::ZapZap,
    }
}

pub fn switch(
    one: Spec,
    mut rep: Vec<Spec>,
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    if rep.is_empty() {
        return relative(axe, &one, dom, hay, cox, &vec![], &None, &None);
    }

    let mut iter = rep.into_iter();
    let i_rep = iter.next().unwrap();
    let t_rep: Vec<Spec> = iter.collect();

    let fin = switch(
        i_rep.clone(),
        t_rep,
        axe,
        mod_,
        dom,
        hay,
        cox,
        bug,
        nut,
        def,
    );

    let example_res = example(&one.clone(), dom, hay, cox, &vec![], &None, &None);

    let fits = Hoon::Fits(
        Box::new(Hoon::TisGal(Box::new(Hoon::Axis(2)), Box::new(example_res))),
        vec![Limb::Axis(peg(axe, 2).expect("+switch, peg failed!"))],
    );

    let relative_result = relative(axe, &one, dom, hay, cox, &vec![], &None, &None);

    Hoon::WutCol(Box::new(fits), Box::new(relative_result), Box::new(fin))
}

pub fn choice_(
    one: Spec,
    mut rep: Vec<Spec>,
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    if rep.is_empty() {
        return relative(axe, &one, dom, hay, cox, &vec![], &None, &None);
    }

    let mut iter = rep.into_iter();
    let i_rep = iter.next().unwrap();
    let t_rep: Vec<Spec> = iter.collect();

    let example_res = example(&one.clone(), dom, hay, cox, &vec![], &None, &None);

    let fits = Hoon::Fits(Box::new(example_res), vec![Limb::Axis(axe)]);

    let relative_result = relative(axe, &one.clone(), dom, hay, cox, &vec![], &None, &None);
    let tail = choice_(
        i_rep.clone(),
        t_rep,
        axe,
        mod_,
        dom,
        hay,
        cox,
        bug,
        nut,
        def,
    );

    Hoon::WutCol(Box::new(fits), Box::new(relative_result), Box::new(tail))
}

pub fn relative(
    axe: u64,
    mod_: &Spec,
    dom: u64,
    hay: &WingType,
    cox: &HashMap<String, Spec>,
    bug: &Vec<Spot>,
    nut: &Option<Note>,
    def: &Option<Hoon>,
) -> Hoon {
    match &mod_ {
        Spec::Base(p) => decorate(
            basic(p.clone(), axe, mod_, dom, hay, cox, &vec![], &None, &None),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Dbug(p, q) => {
            let mut bug = bug.clone();
            bug.push(p.clone());
            relative(axe, &*q, dom, hay, cox, &bug, nut, def)
        }
        Spec::Leaf(p, q) => decorate(
            Hoon::WutGar(
                Box::new(Hoon::DotTis(
                    Box::new(Hoon::Axis(axe)),
                    Box::new(Hoon::Rock("$".to_string(), NounExpr::ParsedAtom(q.clone()))),
                )),
                Box::new(Hoon::Rock(p.clone(), NounExpr::ParsedAtom(q.clone()))),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Make(p, q) => relative(
            axe,
            &Spec::BucMic(unfold(p.clone(), q.clone())),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::Like(p, q) => relative(
            axe,
            &Spec::BucMic(unreel(p.clone(), q.clone())),
            dom,
            hay,
            cox,
            bug,
            nut,
            def,
        ),
        Spec::Loop(p) => decorate(
            Hoon::CenHep(Box::new(Hoon::Limb(p.clone())), Box::new(Hoon::Axis(axe))),
            bug.clone(),
            nut.clone(),
        ),
        Spec::Name(p, q) => relative(
            axe,
            &*q,
            dom,
            hay,
            cox,
            bug,
            &Some(Note::Made(p.clone(), None)),
            def,
        ),
        Spec::Made((term, list), q) => {
            let pieces = list
                .iter()
                .map(|s| vec![Limb::Term(s.to_string())])
                .collect();
            let nut = Some(Note::Made(term.clone(), Some(pieces)));
            relative(axe, &*q, dom, hay, cox, bug, &nut, def)
        }
        Spec::Over(p, q) => relative(axe, &*q, dom, p, cox, bug, nut, def),
        Spec::BucBuc(p, q) => {
            let new_dom = peg(3, dom).expect("+relative-bucbuc-peg-failed");
            let map: HashMap<String, Hoon> = q
                .into_iter()
                .map(|(term, spec)| {
                    (
                        term.clone(),
                        relative(axe, spec, new_dom, hay, cox, bug, nut, def),
                    )
                })
                .collect();
            Hoon::BarKet(
                Box::new(relative(axe, &*p, new_dom, hay, cox, bug, nut, def)),
                HashMap::from([("$".to_string(), (None, map))]),
            )
        }
        Spec::BucPam(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*p, dom, hay, cox, bug, nut, def)),
            Box::new(Hoon::TisLus(
                Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(q.clone()))),
                Box::new(Hoon::TisLus(
                    Box::new(Hoon::CenHep(
                        Box::new(Hoon::Axis(2)),
                        Box::new(Hoon::Axis(6)),
                    )),
                    Box::new(Hoon::WutGar(
                        Box::new(Hoon::WutBar(vec![
                            Hoon::DotTis(Box::new(Hoon::Axis(14)), Box::new(Hoon::Axis(2))),
                            Hoon::DotTis(
                                Box::new(Hoon::Axis(2)),
                                Box::new(Hoon::CenHep(
                                    Box::new(Hoon::Axis(6)),
                                    Box::new(Hoon::Axis(2)),
                                )),
                            ),
                        ])),
                        Box::new(Hoon::Axis(2)),
                    )),
                )),
            )),
        ),
        Spec::BucBar(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*p, dom, hay, cox, bug, nut, def)),
            Box::new(Hoon::WutGar(
                Box::new(Hoon::CenHep(
                    Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(q.clone()))),
                    Box::new(Hoon::Axis(2)),
                )),
                Box::new(Hoon::Axis(2)),
            )),
        ),
        Spec::BucCab(p) => decorate(
            home(p.clone(), hay.clone(), dom.clone()),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucCen(p, t) => decorate(
            switch(
                *p.clone(),
                t.clone(),
                axe,
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucCol(p, q) => {
            let mut result: Option<Hoon> = None;
            let mut current_axe = axe;

            let first = relative(
                peg(current_axe, 2).expect("+relative-buccol-peg-failed"),
                &*p,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );

            result = Some(first);
            current_axe = peg(current_axe, 3).expect("+relative-buccol-peg-failed");

            for spec in q {
                let hoon = relative(
                    peg(current_axe, 2).expect("+relative-buccol-peg-failed"),
                    spec,
                    dom,
                    hay,
                    cox,
                    bug,
                    nut,
                    def,
                );

                result = Some(Hoon::Pair(Box::new(result.unwrap()), Box::new(hoon)));

                current_axe = peg(current_axe, 3).expect("+relative-buccol-peg-failed");
            }

            decorate(result.unwrap(), bug.clone(), nut.clone())
        }
        Spec::BucGal(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
            Box::new(Hoon::WutGal(
                Box::new(Hoon::WutTis(
                    Box::new(Spec::Over(vec![Limb::Axis(3)], p.clone())),
                    vec![Limb::Axis(4)],
                )),
                Box::new(Hoon::Axis(2)),
            )),
        ),
        Spec::BucGar(p, q) => Hoon::TisLus(
            Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
            Box::new(Hoon::WutGar(
                Box::new(Hoon::WutTis(
                    Box::new(Spec::Over(vec![Limb::Axis(3)], p.clone())),
                    vec![Limb::Axis(4)],
                )),
                Box::new(Hoon::Axis(2)),
            )),
        ),
        Spec::BucHep(p, q) => {
            let function_res = function(
                *p.clone(),
                *q.clone(),
                mod_,
                dom,
                hay,
                cox,
                &vec![],
                &None,
                &None,
            );
            decorate(
                match def {
                    Some(d) => Hoon::KetLus(Box::new(function_res), Box::new(d.clone())),
                    None => function_res,
                },
                bug.clone(),
                nut.clone(),
            )
        }
        Spec::BucKet(p, q) => decorate(
            Hoon::WutCol(
                Box::new(Hoon::DotWut(Box::new(Hoon::Axis(
                    peg(axe, 2).expect("bucket-peg-failed"),
                )))),
                Box::new(relative(axe, &*p, dom, hay, cox, &vec![], &None, &None)),
                Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucMic(p) => decorate(
            Hoon::CenCol(
                Box::new(home(p.clone(), hay.clone(), dom.clone())),
                vec![Hoon::Axis(axe)],
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucSig(p, q) => relative(
            axe,
            &*q,
            dom,
            hay,
            cox,
            bug,
            nut,
            &Some(Hoon::KetHep(q.clone(), Box::new(p.clone()))),
        ),
        Spec::BucWut(p, t) => decorate(
            choice_(
                *p.clone(),
                t.clone(),
                axe,
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucTis(p, q) => Hoon::KetTis(
            p.clone(),
            Box::new(relative(axe, &*q, dom, hay, cox, bug, nut, def)),
        ),
        Spec::BucPat(p, q) => decorate(
            Hoon::WutCol(
                Box::new(Hoon::DotWut(Box::new(Hoon::Axis(axe)))),
                Box::new(relative(axe, &*q, dom, hay, cox, &vec![], &None, &None)),
                Box::new(relative(axe, &*p, dom, hay, cox, &vec![], &None, &None)),
            ),
            bug.clone(),
            nut.clone(),
        ),
        Spec::BucLus(p, q) => Hoon::Note(
            Note::Know(p.clone()),
            Box::new(relative(axe, &*q, dom, hay, cox, bug, nut, def)),
        ),
        Spec::BucDot(p, q) => {
            let x = interface(
                Vair::Gold,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }

        Spec::BucFas(p, q) => {
            let x = interface(
                Vair::Iron,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }

        Spec::BucZap(p, q) => {
            let x = interface(
                Vair::Lead,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }

        Spec::BucTic(p, q) => {
            let x = interface(
                Vair::Zinc,
                *p.clone(),
                q.clone(),
                mod_,
                dom,
                hay,
                cox,
                bug,
                nut,
                def,
            );
            let y = home(x, hay.clone(), dom.clone());
            decorate(y, bug.clone(), nut.clone())
        }
    }
}

pub fn home(gen: Hoon, mut hay: WingType, dom: u64) -> Hoon {
    let wing = if 1 != dom {
        hay
    } else {
        hay.push(Limb::Axis(dom));
        hay
    };

    if wing.is_empty() {
        gen
    } else {
        Hoon::TisGar(Box::new(Hoon::Wing(wing)), Box::new(gen))
    }
}

pub fn unreel(one: WingType, res: Vec<WingType>) -> Hoon {
    if res.is_empty() {
        Hoon::Wing(one)
    } else {
        match res.first() {
            Some(first) => {
                let wing_tail = unreel(first.clone(), res[1..].to_vec());
                Hoon::TisGal(Box::new(Hoon::Wing(one)), Box::new(wing_tail))
            }
            None => Hoon::Wing(one),
        }
    }
}

pub fn unfold(fun: Hoon, arg: Vec<Spec>) -> Hoon {
    let cencol_tail: Vec<Hoon> = arg
        .iter()
        .map(|spec| Hoon::KetCol(Box::new(spec.clone())))
        .collect();
    Hoon::CenCol(Box::new(fun), cencol_tail)
}

pub fn factory(
    mod_: Spec,
    dom: u64,
    hay: WingType,
    cox: HashMap<String, Spec>,
    bug: Vec<Spot>,
    nut: Option<Note>,
    def: Option<Hoon>,
) -> Hoon {
    match mod_ {
        Spec::Dbug(spot, spec) => {
            let mut bug = bug.clone();
            bug.insert(0, spot);
            factory(*spec, dom, hay, cox, bug, nut, def)
        }
        Spec::BucSig(hoon, spec) => {
            let spec_clone = spec.clone();
            let spec_clone2 = spec.clone();
            factory(
                *spec_clone,
                dom,
                hay,
                cox,
                bug,
                nut,
                Some(Hoon::KetHep(spec_clone2, Box::new(hoon))),
            )
        }
        _ => match (def.clone(), mod_.clone()) {
            (Some(_), Spec::BucMic(h)) => decorate(home(h, hay, dom), bug, nut),
            (Some(_), Spec::Like(wing, vec_wing)) => {
                decorate(home(unreel(wing, vec_wing), hay, dom), bug, nut)
            }
            (Some(_), Spec::Loop(term)) => decorate(home(Hoon::Limb(term), hay, dom), bug, nut),
            (Some(_), Spec::Make(h, s)) => decorate(home(unfold(h, s), hay, dom), bug, nut),
            _ => {
                let spore_res = spore(
                    mod_.clone(),
                    dom.clone(),
                    hay.clone(),
                    cox.clone(),
                    bug.clone(),
                    nut.clone(),
                    def.clone(),
                );

                let ketsig = Box::new(Hoon::KetSig(Box::new(spore_res)));

                let descent_axis = peg(7, dom).expect("factory-peg-failed");
                let tislus = Hoon::TisLus(
                    Box::new(Hoon::DotTis(
                        Box::new(Hoon::Axis(14)),
                        Box::new(Hoon::Axis(2)),
                    )),
                    Box::new(Hoon::Axis(6)),
                );
                let relative_res = relative(6, &mod_, descent_axis, &hay, &cox, &bug, &nut, &def);
                let tail = Hoon::TisLus(Box::new(relative_res), Box::new(tislus));

                Hoon::BarCol(ketsig, Box::new(tail))
            }
        },
    }
}

pub fn open(gen: Hoon) -> Hoon {
    match gen {
        Hoon::Axis(a) => Hoon::CenTis(vec![Limb::Axis(a)], Vec::new()),
        Hoon::Base(b) => factory(
            Spec::Base(b),
            1,
            Vec::new(),
            HashMap::new(),
            Vec::new(),
            None,
            None,
        ),
        Hoon::Bust(b) => example(
            &Spec::Base(b),
            1,
            &WingType::default(),
            &HashMap::new(),
            &Vec::new(),
            &None,
            &None,
        ),
        Hoon::Dbug(_, q) => *q,
        Hoon::Eror(s) => panic!("{}", s),
        Hoon::Knit(woofs) => {
            let ktts = Hoon::KetTis(Skin::Term("v".to_string()), Box::new(Hoon::Axis(1)));

            fn knit_loop(woofs: Vec<Woof>) -> Hoon {
                if woofs.is_empty() {
                    Hoon::Bust(BaseType::Null)
                } else {
                    let head = &woofs[0];
                    let tail = knit_loop(woofs[1..].to_vec());
                    match head {
                        Woof::ParsedAtom(a) => {
                            let sand =
                                Hoon::Sand("tD".to_string(), NounExpr::ParsedAtom(a.clone()));
                            Hoon::Pair(Box::new(sand), Box::new(tail))
                        }
                        Woof::Hoon(p) => {
                            let a = Hoon::Pair(
                                Box::new(Hoon::KetTis(
                                    Skin::Term("a".to_string()),
                                    Box::new(Hoon::KetLus(
                                        Box::new(Hoon::Limb("$".to_string())),
                                        Box::new(Hoon::TisGar(
                                            Box::new(Hoon::Limb("v".to_string())),
                                            Box::new(p.clone()),
                                        )),
                                    )),
                                )),
                                Box::new(Hoon::KetTis(Skin::Term("a".to_string()), Box::new(tail))),
                            );
                            let b = Hoon::BarHep(Box::new(Hoon::WutPat(
                                vec![Limb::Term("a".to_string())],
                                Box::new(Hoon::Limb("b".to_string())),
                                Box::new(Hoon::Pair(
                                    Box::new(Hoon::TisGal(
                                        Box::new(Hoon::Axis(2)),
                                        Box::new(Hoon::Limb("a".to_string())),
                                    )),
                                    Box::new(Hoon::CenTis(
                                        vec![Limb::Term("$".to_string())],
                                        vec![(
                                            vec![Limb::Term("a".to_string())],
                                            Hoon::TisGal(
                                                Box::new(Hoon::Axis(3)),
                                                Box::new(Hoon::Limb("a".to_string())),
                                            ),
                                        )],
                                    )),
                                )),
                            )));

                            Hoon::TisLus(Box::new(a), Box::new(b))
                        }
                    }
                }
            }

            let ktls = Hoon::KetLus(
                Box::new(Hoon::BarHep(Box::new(Hoon::WutCol(
                    Box::new(Hoon::Bust(BaseType::Flag)),
                    Box::new(Hoon::Bust(BaseType::Null)),
                    Box::new(Hoon::Pair(
                        Box::new(Hoon::KetTis(
                            Skin::Term("i".to_string()),
                            Box::new(Hoon::Sand(
                                "tD".to_string(),
                                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                            )),
                        )),
                        Box::new(Hoon::KetTis(
                            Skin::Term("t".to_string()),
                            Box::new(Hoon::Limb("$".to_string())),
                        )),
                    )),
                )))),
                Box::new(knit_loop(woofs)),
            );

            let brhp = Hoon::BarHep(Box::new(ktls));

            Hoon::TisGar(Box::new(ktts), Box::new(brhp))
        }
        Hoon::Leaf(term, atom) => factory(
            Spec::Leaf(term, atom),
            1,
            Vec::new(),
            HashMap::new(),
            Vec::new(),
            None,
            None,
        ),
        Hoon::Limb(term) => Hoon::CenTis(vec![Limb::Term(term)], Vec::new()),
        Hoon::Wing(wing) => Hoon::CenTis(wing, Vec::new()),
        Hoon::Note(_, q) => *q,

        Hoon::Tell(hoons) => {
            let zpgr = Hoon::ZapGar(Box::new(Hoon::ColTar(hoons)));
            Hoon::CenCol(Box::new(Hoon::Limb("noah".to_string())), vec![zpgr])
        }

        Hoon::Yell(hoons) => {
            let zpgr = Hoon::ZapGar(Box::new(Hoon::ColTar(hoons)));
            Hoon::CenCol(Box::new(Hoon::Limb("cain".to_string())), vec![zpgr])
        }

        Hoon::BarBuc(sample, body) => {
            if sample.is_empty() {
                panic!("empty sample in BarBuc");
            }

            let tar = Spec::Base(BaseType::NounExpr);
            let bcsg = Spec::BucSig(
                Hoon::Base(BaseType::NounExpr),
                Box::new(Spec::BucHep(Box::new(tar.clone()), Box::new(tar))),
            );

            let transformed: Vec<Spec> = sample
                .iter()
                .map(|term| Spec::BucTis(Skin::Term(term.clone()), Box::new(bcsg.clone())))
                .collect();

            let (first, rest) = transformed.split_first().unwrap();

            Hoon::BarTar(
                Box::new(Spec::BucCol(Box::new(first.clone()), rest.to_vec())),
                Box::new(Hoon::KetCol(Box::new(*body))),
            )
        }

        Hoon::BarCab(spec, alas, arms) => {
            let transformed_arms = arms
                .into_iter()
                .map(|(term, tome)| {
                    let (what, tome_map) = tome;
                    let wrapped_pairs: Vec<(String, Hoon)> = tome_map
                        .into_iter()
                        .map(|(face, expr)| {
                            let wrapped_expr =
                                alas.iter()
                                    .rev()
                                    .fold(expr, |body, (alas_face, alas_init)| {
                                        Hoon::TisTar(
                                            (alas_face.clone(), None),
                                            Box::new(alas_init.clone()),
                                            Box::new(body),
                                        )
                                    });
                            (face, wrapped_expr)
                        })
                        .collect();

                    let tome_map: HashMap<_, _> = wrapped_pairs.into_iter().collect();

                    (term, (what, tome_map))
                })
                .collect();

            Hoon::TisLus(
                Box::new(Hoon::KetTar(spec)),
                Box::new(Hoon::BarCen(None, transformed_arms)),
            )
        }

        Hoon::BarCol(p, q) => Hoon::TisLus(p, Box::new(Hoon::BarDot(q))),

        Hoon::BarDot(p) => {
            let map_term_hoon = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), *p);
                m
            };
            let map_term_tome = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), (None, map_term_hoon));
                m
            };
            Hoon::BarCen(None, map_term_tome)
        }

        Hoon::BarKet(p, arms) => {
            let mut map = arms.clone();
            if let Some(zil) = arms.get(&"$".to_string()) {
                let updated = {
                    let (what, mut inner) = zil.clone();
                    inner.insert("$".to_string(), *p.clone());
                    (what, inner)
                };
                map.insert("$".to_string(), updated);
            } else {
                let mut inner = HashMap::new();
                inner.insert("$".to_string(), *p.clone());
                map.insert("$".to_string(), (None, inner));
            }
            Hoon::TisGal(
                Box::new(Hoon::Limb("$".to_string())),
                Box::new(Hoon::BarCen(None, map)),
            )
        }

        Hoon::BarHep(p) => Hoon::TisGal(
            Box::new(Hoon::Limb("$".to_string())),
            Box::new(Hoon::BarDot(Box::new(*p))),
        ),

        Hoon::BarSig(spec, q) => Hoon::KetBar(Box::new(Hoon::BarTis(spec.clone(), q.clone()))),

        Hoon::BarTar(spec, q) => {
            let map_term_hoon = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), *q);
                m
            };
            let map_term_tome = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), (None, map_term_hoon));
                m
            };
            Hoon::TisLus(
                Box::new(Hoon::KetTar(spec)),
                Box::new(Hoon::BarPat(None, map_term_tome)),
            )
        }

        Hoon::BarTis(spec, q) => {
            let map_term_hoon = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), *q);
                m
            };
            let map_term_tome = {
                let mut m = HashMap::new();
                m.insert("$".to_string(), (None, map_term_hoon));
                m
            };
            Hoon::BarCab(spec, vec![], map_term_tome)
        }

        Hoon::BarWut(p) => Hoon::KetWut(Box::new(Hoon::BarDot(p))),

        Hoon::ColKet(p, q, r, s) => {
            Hoon::Pair(p, Box::new(Hoon::Pair(q, Box::new(Hoon::Pair(r, s)))))
        }

        Hoon::ColCab(p, q) => Hoon::Pair(q, p),

        Hoon::ColHep(p, q) => Hoon::Pair(p, q),

        Hoon::ColLus(p, q, r) => Hoon::Pair(p, Box::new(Hoon::Pair(q, r))),

        Hoon::ColSig(hoons) => match hoons.as_slice() {
            [] => Hoon::Rock("n".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
            [h] => h.clone(),
            [h, tail @ ..] => {
                let rest = open(Hoon::ColSig(tail.to_vec()));
                Hoon::Pair(Box::new(h.clone()), Box::new(rest))
            }
        },

        Hoon::ColTar(hoons) => match hoons.as_slice() {
            [] => Hoon::ZapZap,
            [h] => h.clone(),
            [h, tail @ ..] => {
                let rest = open(Hoon::ColTar(tail.to_vec()));
                Hoon::Pair(Box::new(h.clone()), Box::new(rest))
            }
        },
        Hoon::KetTar(spec) => Hoon::KetSig(Box::new(example(
            &spec,
            1,
            &Vec::new(),
            &HashMap::new(),
            &Vec::new(),
            &None,
            &None,
        ))),

        Hoon::CenCab(wing, pairs) => Hoon::KetLus(
            Box::new(Hoon::Wing(wing.clone())),
            Box::new(Hoon::CenTis(wing, pairs)),
        ),

        Hoon::CenDot(p, q) => Hoon::CenCol(q, vec![*p]),

        Hoon::CenKet(p, q, r, s) => Hoon::CenCol(p, vec![*q, *r, *s]),

        Hoon::CenLus(p, q, r) => Hoon::CenCol(p, vec![*q, *r]),

        Hoon::CenHep(p, q) => Hoon::CenCol(p, vec![*q]),

        Hoon::CenCol(p, hoons) => Hoon::CenSig(vec![Limb::Term("$".to_string())], p, hoons),

        Hoon::CenSig(wing, p, hoons) => {
            fn compile_r_gen_rec(r_gen: &[Hoon], axe: u64) -> Vec<(Vec<Limb>, Hoon)> {
                match r_gen.split_first() {
                    None => vec![],
                    Some((hoon, rest)) => {
                        let (wing_axe, next_axe) = if rest.is_empty() {
                            (axe, 0)
                        } else {
                            (
                                peg(axe, 2).expect("+open: peg failed"),
                                peg(axe, 3).expect("+open: peg failed"),
                            )
                        };

                        let wing = vec![Limb::Parent(0, None), Limb::Axis(wing_axe)];

                        let mut out = vec![(wing, hoon.clone())];
                        if !rest.is_empty() {
                            out.extend(compile_r_gen_rec(rest, next_axe));
                        }
                        out
                    }
                }
            }
            let list = compile_r_gen_rec(&hoons, 6);
            Hoon::CenTar(wing, p, list)
        }

        Hoon::CenTar(mut wing, p, pairs) => {
            if pairs.is_empty() {
                return Hoon::TisGar(p, Box::new(Hoon::Wing(wing)));
            }
            wing.extend(vec![Limb::Axis(2)]);
            let wrapped = pairs
                .into_iter()
                .map(|(p, q)| (p, Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(q))))
                .collect();
            Hoon::TisLus(p, Box::new(Hoon::CenTis(wing, wrapped)))
        }

        Hoon::KetDot(p, q) => Hoon::KetLus(Box::new(Hoon::CenCol(p, vec![*q.clone()])), q),

        Hoon::KetHep(spec, q) => {
            let example_res = example(
                &spec,
                1,
                &Vec::new(),
                &HashMap::new(),
                &Vec::new(),
                &None,
                &None,
            );
            Hoon::KetLus(Box::new(example_res), q)
        }

        Hoon::KetTis(skin, p) => grip(skin, *p, vec![]),

        Hoon::SigBar(p, q) => {
            let fek = {
                let fek = feck(*p.clone());
                match fek {
                    Some(s) => Hoon::Rock("tas".to_string(), NounExpr::ParsedAtom(s)),
                    None => Hoon::BarDot(Box::new(Hoon::CenCol(
                        Box::new(Hoon::Limb("cain".to_string())),
                        vec![Hoon::ZapGar(Box::new(Hoon::TisGal(Box::new(Hoon::Axis(3)), p)))],
                    ))),
                }
            };
            let hint = TermOrPair::Pair("mean".to_string(), Box::new(fek));
            Hoon::SigGar(hint, q)
        }

        Hoon::SigCab(p, q) => Hoon::SigGar(
            TermOrPair::Term("mean".to_string()),
            Box::new(Hoon::BarDot(p)),
        ),

        Hoon::SigCen(chum, p, tyre, q) => {
            let clsg_vec = {
                let mut nob = vec![];
                let mut r = tyre;
                while !r.is_empty() {
                    let (p_i, q_i) = r.remove(0);
                    nob.push(Hoon::Pair(
                        Box::new(Hoon::Rock(
                            "$".to_string(),
                            NounExpr::ParsedAtom(string_to_atom(p_i)),
                        )),
                        Box::new(Hoon::ZapTis(Box::new(q_i))),
                    ));
                }
                nob
            };
            let clls = Hoon::ColLus(
                Box::new(Hoon::Rock("$".to_string(), chum_to_nounexpr(chum))),
                Box::new(Hoon::ZapTis(q.clone())),
                Box::new(Hoon::ColSig(clsg_vec)),
            );
            Hoon::SigGal(TermOrPair::Pair("fast".to_string(), Box::new(clls)), q)
        }

        Hoon::SigFas(chum, q) => Hoon::SigCen(chum, Box::new(Hoon::Axis(7)), vec![], q),

        Hoon::SigGal(term_or_pair, q) => Hoon::TisGal(
            Box::new(Hoon::SigGar(term_or_pair, Box::new(Hoon::Axis(1)))),
            q,
        ),

        Hoon::SigBuc(term, q) => Hoon::SigGar(
            TermOrPair::Pair(
                "live".to_string(),
                Box::new(Hoon::Rock(
                    "$".to_string(),
                    NounExpr::ParsedAtom(string_to_atom(term)),
                )),
            ),
            q,
        ),

        Hoon::SigLus(a, q) => Hoon::SigGar(
            TermOrPair::Pair(
                "memo".to_string(),
                Box::new(Hoon::Rock(
                    "$".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(a.into())),
                )),
            ),
            q,
        ),

        Hoon::SigPam(a, p, q) => Hoon::SigGar(
            TermOrPair::Pair(
                "slog".to_string(),
                Box::new(Hoon::Pair(
                    Box::new(Hoon::Sand(
                        "$".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(a.into())),
                    )),
                    Box::new(Hoon::CenCol(
                        Box::new(Hoon::Limb("cain".to_string())),
                        vec![Hoon::ZapGar(p)],
                    )),
                )),
            ),
            q,
        ),

        Hoon::SigTis(p, q) => Hoon::SigGar(TermOrPair::Pair("germ".to_string(), p), q),

        Hoon::SigWut(a, p, q, r) => {
            let wtdt = Hoon::WutDot(
                p,
                Box::new(Hoon::Bust(BaseType::Null)),
                Box::new(Hoon::Pair(
                    Box::new(Hoon::Bust(BaseType::Null)),
                    Box::new(*q),
                )),
            );
            let sgpm = Hoon::SigPam(
                a,
                Box::new(Hoon::Axis(5)),
                Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), r.clone())),
            );
            let wtsg = Hoon::WutSig(
                vec![Limb::Axis(2)],
                Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), r)),
                Box::new(sgpm),
            );
            Hoon::TisLus(Box::new(wtdt), Box::new(wtsg))
        }

        Hoon::MicTis(marl) => {
            fn loop_marl(marl: Marl) -> Hoon {
                match marl.split_first() {
                    None => Hoon::Bust(BaseType::Null),
                    Some((head, tail)) => match head {
                        Tuna::Manx(m) => Hoon::Pair(
                            Box::new(Hoon::Xray(m.clone())),
                            Box::new(loop_marl(tail.to_vec())),
                        ),
                        Tuna::TunaTail(TunaTail::Manx(m)) => {
                            Hoon::Pair(Box::new(m.clone()), Box::new(loop_marl(tail.to_vec())))
                        }
                        Tuna::TunaTail(TunaTail::Tape(t)) => Hoon::Pair(
                            Box::new(Hoon::MicFas(Box::new(t.clone()))),
                            Box::new(loop_marl(tail.to_vec())),
                        ),
                        Tuna::TunaTail(TunaTail::Call(h)) => {
                            Hoon::CenCol(Box::new(h.clone()), vec![loop_marl(tail.to_vec())])
                        }
                        Tuna::TunaTail(TunaTail::Marl(sub)) => {
                            let tsbr = Box::new(Hoon::TisBar(
                                Box::new(Spec::Base(BaseType::Cell)),
                                Box::new(Hoon::BarPat(None, {
                                    let sug = vec![Limb::Axis(12)];
                                    let wtsg = Hoon::WutSig(
                                        sug.clone(),
                                        Box::new(Hoon::CenTis(
                                            sug.clone(),
                                            vec![(vec![Limb::Axis(1)], Hoon::Axis(13))],
                                        )),
                                        Box::new(Hoon::CenTis(
                                            sug.clone(),
                                            vec![(
                                                vec![Limb::Axis(3)],
                                                Hoon::CenTis(
                                                    vec![Limb::Term("$".to_string())],
                                                    vec![(sug, Hoon::Axis(25))],
                                                ),
                                            )],
                                        )),
                                    );
                                    let map_term_hoon = {
                                        let mut m = HashMap::new();
                                        m.insert("$".to_string(), wtsg);
                                        m
                                    };
                                    let map_term_tome = {
                                        let mut m = HashMap::new();
                                        m.insert("$".to_string(), (None, map_term_hoon));
                                        m
                                    };
                                    map_term_tome
                                })),
                            ));
                            Hoon::CenDot(
                                Box::new(Hoon::Pair(
                                    Box::new(sub.clone()),
                                    Box::new(loop_marl(tail.to_vec())),
                                )),
                                tsbr,
                            )
                        }
                    },
                }
            }
            loop_marl(marl)
        }

        Hoon::MicCol(p, hoons) => match hoons.as_slice() {
            [] => Hoon::ZapZap,
            [h] => h.clone(),
            [h, tail @ ..] => {
                let yex = hoons;
                fn loop_yex(yex: &[Hoon]) -> Hoon {
                    match yex {
                        [] => panic!("empty yex"),
                        [h] => Hoon::TisGal(Box::new(Hoon::Axis(3)), Box::new(h.clone())),
                        [h, t @ ..] => Hoon::CenCol(
                            Box::new(Hoon::Axis(2)),
                            vec![
                                Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(h.clone())),
                                loop_yex(t),
                            ],
                        ),
                        _ => panic!("miccol error"),
                    }
                }
                Hoon::TisLus(p, Box::new(loop_yex(&yex)))
            }
        },

        Hoon::MicFas(p) => {
            let zoy = Hoon::Rock("ta".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0)));
            Hoon::ColSig(vec![Hoon::Pair(
                Box::new(zoy.clone()),
                Box::new(Hoon::ColSig(vec![Hoon::Pair(
                    Box::new(zoy.clone()),
                    p.clone(),
                )])),
            )])
        }

        Hoon::MicGal(spec, q, r, s) => {
            let ktcl_p = Hoon::KetCol(spec.clone());
            let cnhp = Hoon::CenHep(q, Box::new(ktcl_p));
            let brts = Hoon::BarTis(spec, Box::new(Hoon::TisGar(Box::new(Hoon::Axis(3)), s)));
            Hoon::CenLus(Box::new(cnhp), r, Box::new(brts))
        }

        Hoon::MicSig(p, q) => {
            fn loop_tail(p: Box<Hoon>, q: Vec<Hoon>) -> Hoon {
                match q.as_slice() {
                    [] => {
                        panic!("open-mcsg")
                    }
                    [first, rest @ ..] => {
                        if rest.is_empty() {
                            return Hoon::TisGar(
                                Box::new(Hoon::Limb("v".to_string())),
                                Box::new(first.clone()),
                            );
                        }
                        let a_bind = Hoon::KetTis(
                            Skin::Term("a".to_string()),
                            Box::new(loop_tail(p.clone(), rest.to_vec())),
                        );

                        let b_expr = Hoon::TisGar(
                            Box::new(Hoon::Limb("v".to_string())),
                            Box::new(first.clone()),
                        );
                        let b_bind = Hoon::KetTis(
                            Skin::Term("b".to_string()),
                            Box::new(Hoon::TisGar(
                                Box::new(Hoon::Limb("v".to_string())),
                                Box::new(first.clone()),
                            )),
                        );

                        let wing_c = vec![Limb::Parent(0, None), Limb::Axis(6)];
                        let c_expr = Hoon::TisGal(
                            Box::new(Hoon::Wing(wing_c)),
                            Box::new(Hoon::Limb("b".to_string())),
                        );
                        let c_bind = Hoon::KetTis(
                            Skin::Term("c".to_string()),
                            Box::new(Hoon::TisGal(
                                Box::new(Hoon::Wing(vec![Limb::Parent(0, None), Limb::Axis(6)])),
                                Box::new(Hoon::Limb("b".to_string())),
                            )),
                        );

                        let tsgr_v_p =
                            Hoon::TisGar(Box::new(Hoon::Limb("v".to_string())), p.clone());
                        let cncl_b_c = Hoon::CenCol(
                            Box::new(Hoon::Limb("b".to_string())),
                            vec![Hoon::Limb("c".to_string())],
                        );
                        let cnts_wing = vec![Limb::Parent(0, None), Limb::Axis(6)];
                        let cnts = Hoon::CenTis(
                            vec![Limb::Term("a".to_string())],
                            vec![(cnts_wing, Hoon::Limb("c".to_string()))],
                        );
                        let cnls =
                            Hoon::CenLus(Box::new(tsgr_v_p), Box::new(cncl_b_c), Box::new(cnts));

                        Hoon::TisLus(
                            Box::new(a_bind),
                            Box::new(Hoon::TisLus(
                                Box::new(b_bind),
                                Box::new(Hoon::TisLus(
                                    Box::new(c_bind),
                                    Box::new(Hoon::BarDot(Box::new(cnls))),
                                )),
                            )),
                        )
                    }
                }
            };

            let tail = loop_tail(p, q);

            Hoon::TisGar(
                Box::new(Hoon::KetTis(
                    Skin::Term("$".to_string()),
                    Box::new(Hoon::Axis(1)),
                )),
                Box::new(tail),
            )
        }

        Hoon::MicMic(spec, q) => Hoon::CenHep(
            Box::new(factory(
                *spec,
                1,
                Vec::new(),
                HashMap::new(),
                Vec::new(),
                None,
                None,
            )),
            q,
        ),

        Hoon::TisBar(spec, q) => Hoon::TisLus(Box::new(Hoon::KetTar(spec)), q),

        Hoon::TisTar((term, spec_opt), p, q) => {
            let inner = match spec_opt {
                None => *p,
                Some(spec_box) => Hoon::KetHep(spec_box, p),
            };
            let mut m = HashMap::new();
            m.insert(term, Some(inner));
            Hoon::TisGal(q, Box::new(Hoon::Tune(TermOrTune::Tune((m, vec![])))))
        }

        Hoon::TisCol(pairs, q) => {
            let wing = vec![Limb::Term("$".to_string())];
            Hoon::TisGar(Box::new(Hoon::CenCab(wing, pairs)), q)
        }

        Hoon::TisFas(skin, p, q) => Hoon::TisLus(Box::new(Hoon::KetTis(skin, p)), q),

        Hoon::TisMic(skin, p, q) => Hoon::TisFas(skin, q, p),

        Hoon::TisDot(wing, p, q) => Hoon::TisGar(
            Box::new(Hoon::CenCab(vec![Limb::Axis(1)], vec![(wing, *p)])),
            q,
        ),

        Hoon::TisWut(wing, p, q, r) => {
            let wtcl = Hoon::WutCol(p, q, Box::new(Hoon::Wing(wing.clone())));
            Hoon::TisDot(wing, Box::new(wtcl), r)
        }

        Hoon::TisGal(p, q) => Hoon::TisGar(q, p),

        Hoon::TisHep(p, q) => Hoon::TisLus(q, p),

        Hoon::TisKet(skin, wing, p, q) => {
            let wuy = weld(wing.clone(), vec![Limb::Term("v".to_string())]);
            let v_bind = Hoon::KetTis(Skin::Term("v".to_string()), Box::new(Hoon::Axis(1)));
            let a_bind = Hoon::KetTis(
                Skin::Term("a".to_string()),
                Box::new(Hoon::TisGar(
                    Box::new(Hoon::Limb("v".to_string())),
                    p.clone(),
                )),
            );
            let tsdt = Box::new(Hoon::TisDot(
                wuy.clone(),
                Box::new(Hoon::TisGal(
                    Box::new(Hoon::Axis(3)),
                    Box::new(Hoon::Limb("a".to_string())),
                )),
                Box::new(Hoon::TisGar(
                    Box::new(Hoon::Pair(
                        Box::new(Hoon::KetTis(
                            Skin::Over(vec![Limb::Term("v".to_string())], Box::new(skin)),
                            Box::new(Hoon::TisGal(
                                Box::new(Hoon::Axis(2)),
                                Box::new(Hoon::Limb("a".to_string())),
                            )),
                        )),
                        Box::new(Hoon::Limb("v".to_string())),
                    )),
                    q,
                )),
            ));
            Hoon::TisGar(
                Box::new(v_bind),
                Box::new(Hoon::TisLus(Box::new(a_bind), tsdt)),
            )
        }

        Hoon::TisLus(p, q) => Hoon::TisGar(Box::new(Hoon::Pair(p, Box::new(Hoon::Axis(1)))), q),

        Hoon::TisSig(hoons) => match hoons.as_slice() {
            [] => Hoon::Axis(1),
            [h] => h.clone(),
            [h, tail @ ..] => {
                let rest = open(Hoon::TisSig(tail.to_vec()));
                Hoon::TisGar(Box::new(h.clone()), Box::new(rest))
            }
        },
        Hoon::WutBar(p) => match p.as_slice() {
            [] => Hoon::Rock("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(1))),
            [head, tail @ ..] => {
                let recurse = open(Hoon::WutBar(tail.to_vec()));
                Hoon::WutCol(
                    Box::new(head.clone()),
                    Box::new(Hoon::Rock(
                        "f".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                    )),
                    Box::new(recurse),
                )
            }
        },

        Hoon::WutDot(p, q, r) => Hoon::WutCol(Box::new(*p), r, q),

        Hoon::WutGal(p, q) => Hoon::WutCol(Box::new(*p), Box::new(Hoon::ZapZap), q),

        Hoon::WutGar(p, q) => Hoon::WutCol(Box::new(*p), q, Box::new(Hoon::ZapZap)),

        Hoon::WutKet(p, q, r) => {
            let wuttis = Hoon::WutTis(Box::new(Spec::Base(BaseType::Atom("$".to_string()))), p);
            Hoon::WutCol(Box::new(wuttis), r, q)
        }

        Hoon::WutHep(p, q) => match q.as_slice() {
            [] => Hoon::Lost(Box::new(Hoon::Wing(p))),
            [(spec, head), tail @ ..] => {
                let wtts = Hoon::WutTis(Box::new(spec.clone()), p.clone());
                let recurse = open(Hoon::WutHep(p.clone(), tail.to_vec()));
                Hoon::WutCol(Box::new(wtts), Box::new(head.clone()), Box::new(recurse))
            }
        },

        Hoon::WutLus(p, q, r) => {
            let mut new_r = r.clone();
            new_r.push((Spec::Base(BaseType::NounExpr), *q));
            Hoon::WutHep(p, new_r)
        }

        Hoon::WutPam(p) => match p.as_slice() {
            [] => Hoon::Rock("f".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(0))),
            [head, tail @ ..] => {
                let recurse = open(Hoon::WutPam(tail.to_vec()));
                Hoon::WutCol(
                    Box::new(head.clone()),
                    Box::new(recurse),
                    Box::new(Hoon::Rock(
                        "f".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(1)),
                    )),
                )
            }
        },

        Hoon::Xray(manx) => {
            let open_mane = match &manx.g.n {
                Mane::Tag(s) => Hoon::Rock(
                    "tas".to_string(),
                    NounExpr::ParsedAtom(string_to_atom(s.clone())),
                ),
                Mane::TagSpace(a, b) => {
                    let left = Hoon::Rock(
                        "tas".to_string(),
                        NounExpr::ParsedAtom(string_to_atom(a.clone())),
                    );
                    let right = Hoon::Rock(
                        "tas".to_string(),
                        NounExpr::ParsedAtom(string_to_atom(b.clone())),
                    );
                    Hoon::Pair(Box::new(left), Box::new(right))
                }
            };

            let clsg_items: Vec<Hoon> = manx
                .g
                .a
                .iter()
                .map(|(mane, beers)| {
                    let n_hoon = match &mane {
                        Mane::Tag(s) => Hoon::Rock(
                            "tas".to_string(),
                            NounExpr::ParsedAtom(string_to_atom(s.clone())),
                        ),
                        Mane::TagSpace(a, b) => {
                            let left = Hoon::Rock(
                                "tas".to_string(),
                                NounExpr::ParsedAtom(string_to_atom(a.clone())),
                            );
                            let right = Hoon::Rock(
                                "tas".to_string(),
                                NounExpr::ParsedAtom(string_to_atom(b.clone())),
                            );
                            Hoon::Pair(Box::new(left), Box::new(right))
                        }
                    };
                    let woofs: Vec<Woof> = beers
                        .iter()
                        .map(|b| match b {
                            Beer::Char(cord) => Woof::ParsedAtom(string_to_atom(cord.clone())),
                            Beer::Hoon(hoon) => Woof::Hoon(hoon.clone()),
                        })
                        .collect();

                    Hoon::Pair(Box::new(n_hoon), Box::new(Hoon::Knit(woofs)))
                })
                .collect();

            let clsg = Hoon::ColSig(clsg_items);
            let head = Hoon::Pair(Box::new(open_mane), Box::new(clsg));
            let tail = Hoon::MicTis(manx.c);

            Hoon::Pair(Box::new(head), Box::new(tail))
        }

        Hoon::WutPat(p, q, r) => {
            let wtts = Hoon::WutTis(Box::new(Spec::Base(BaseType::Atom("$".to_string()))), p);
            Hoon::WutCol(Box::new(wtts), q, r)
        }

        Hoon::WutSig(p, q, r) => {
            let wtts = Hoon::WutTis(Box::new(Spec::Base(BaseType::Null)), p);
            Hoon::WutCol(Box::new(wtts), q, r)
        }

        Hoon::WutTis(spec, q) => {
            let example_res = example(
                &spec,
                1,
                &Vec::new(),
                &HashMap::new(),
                &Vec::new(),
                &None,
                &None,
            );
            Hoon::Fits(Box::new(example_res), q)
        }

        Hoon::WutZap(p) => Hoon::WutCol(
            p,
            Box::new(Hoon::Rock(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(1)),
            )),
            Box::new(Hoon::Rock(
                "f".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            )),
        ),

        Hoon::ZapGar(p) => {
            let limb_onan = Hoon::Limb("onan".to_string());
            let limb_abel = Hoon::Limb("abel".to_string());
            let bcmc = Spec::BucMic(limb_abel);
            let kttr = Hoon::KetTar(Box::new(bcmc));
            let zpmc = Hoon::ZapMic(Box::new(kttr), p);

            Hoon::CenCol(Box::new(limb_onan), vec![zpmc])
        }

        Hoon::ZapWut(arg, q) => {
            const HOON_VERSION: u64 = 138; // hardcoded...

            let version_ok = match &arg {
                ZpwtArg::ParsedAtom(s) => s.parse::<u64>().map_or(false, |v| HOON_VERSION <= v),
                ZpwtArg::Pair(min_s, max_s) => match (min_s.parse::<u64>(), max_s.parse::<u64>()) {
                    (Ok(min), Ok(max)) => min <= HOON_VERSION && HOON_VERSION <= max,
                    _ => false,
                },
            };

            if version_ok {
                *q
            } else {
                panic!("hoon-version")
            }
        }

        _ => gen,
    }
}

pub fn chum_to_nounexpr(chum: Chum) -> NounExpr {
    match chum {
        Chum::Lef(term) => NounExpr::ParsedAtom(string_to_atom(term)),
        Chum::StdKel(term, u) => NounExpr::Cell(
            Box::new(NounExpr::ParsedAtom(string_to_atom(term))),
            Box::new(NounExpr::ParsedAtom(u)),
        ),
        Chum::VenProKel(t1, t2, u) => NounExpr::Cell(
            Box::new(NounExpr::ParsedAtom(string_to_atom(t1))),
            Box::new(NounExpr::Cell(
                Box::new(NounExpr::ParsedAtom(string_to_atom(t2))),
                Box::new(NounExpr::ParsedAtom(u)),
            )),
        ),
        Chum::VenProVerKel(t1, t2, u1, u2) => NounExpr::Cell(
            Box::new(NounExpr::ParsedAtom(string_to_atom(t1))),
            Box::new(NounExpr::Cell(
                Box::new(NounExpr::ParsedAtom(string_to_atom(t2))),
                Box::new(NounExpr::Cell(
                    Box::new(NounExpr::ParsedAtom(u1)),
                    Box::new(NounExpr::ParsedAtom(u2)),
                )),
            )),
        ),
    }
}

pub fn flay(gen: Hoon) -> Option<Skin> {
    match gen {
        Hoon::Pair(p, q) => {
            let maybe_p = flay(*p);
            let maybe_q = flay(*q);
            match (maybe_p, maybe_q) {
                (Some(p), Some(q)) => Some(Skin::Cell(Box::new(p), Box::new(q))),
                _ => None,
            }
        }

        Hoon::Base(b) => Some(Skin::Base(b.clone())),

        Hoon::Rock(t, n) => match n {
            NounExpr::ParsedAtom(a) => Some(Skin::Leaf(t.to_string(), a)),
            NounExpr::Cell(_, _) => None,
        },

        Hoon::CenTis(w, l) => match (w, l) {
            (v, l) if l.is_empty() => match v.as_slice() {
                [Limb::Term(t)] => Some(Skin::Term((*t).to_string())),
                _ => None,
            },
            _ => None,
        },

        Hoon::TisGar(p, q) => {
            let maybe_wing = reek(*p);
            match maybe_wing {
                Some(w) => {
                    let skin = flay(*q);
                    match skin {
                        None => None,
                        Some(s) => Some(Skin::Over(w, Box::new(s))),
                    }
                }
                None => None,
            }
        }

        Hoon::Limb(t) => Some(Skin::Term(t.to_string())),

        Hoon::Wing(w) => match w.as_slice() {
            [Limb::Term(t)] => Some(Skin::Term(t.clone())),
            _ => {
                fn recur(w: &[Limb]) -> Option<Skin> {
                    match w {
                        [] => Some(Skin::Wash(0)),
                        [Limb::Parent(0, None), rest @ ..] => recur(rest),
                        _ => None,
                    }
                }
                recur(w.as_slice())
            }
        },

        Hoon::KetTar(s) => Some(Skin::Spec(
            s.clone(),
            Box::new(Skin::Base(BaseType::NounExpr)),
        )),

        Hoon::KetTis(skin, h) => {
            let maybe_skin = flay(*h);
            match maybe_skin {
                Some(s) => match skin {
                    Skin::Term(ref t) => Some(Skin::Name(t.to_string(), Box::new(skin.clone()))),
                    Skin::Name(ref t, ref b) if matches!(**b, Skin::Base(BaseType::NounExpr)) => {
                        Some(Skin::Name(t.clone(), Box::new(s)))
                    }
                    _ => None,
                },
                None => None,
            }
        }

        _ => {
            let desugared = open(gen.clone());
            if desugared == gen {
                None
            } else {
                flay(desugared)
            }
        }
    }
}

pub fn feck(gen: Hoon) -> Option<ParsedAtom> {
    match gen {
        Hoon::Sand(term, noun) => {
            if term == "tas" {
                match noun {
                    NounExpr::ParsedAtom(s) => Some(s),
                    NounExpr::Cell(_, _) => None,
                }
            } else {
                None
            }
        }

        Hoon::Dbug(_spot, expr) => feck(*expr),

        _ => None,
    }
}

pub fn grip(skin: Skin, gen: Hoon, rel: WingType) -> Hoon {
    match skin {
        Skin::Term(term) => {
            Hoon::TisGal(Box::new(Hoon::Tune(TermOrTune::Term(term))), Box::new(gen))
        }

        Skin::Base(base) => {
            if base == BaseType::NounExpr {
                gen
            } else {
                Hoon::KetHep(Box::new(Spec::Base(base)), Box::new(gen))
            }
        }

        Skin::Cell(car_skin, cdr_skin) => {
            let haf = half(gen.clone());
            match haf {
                None => {
                    let car_gen = Hoon::Axis(4);
                    let cdr_gen = Hoon::Axis(5);
                    let pair = Hoon::Pair(
                        Box::new(grip(*car_skin, car_gen, rel.clone())),
                        Box::new(grip(*cdr_skin, cdr_gen, rel.clone())),
                    );
                    Hoon::TisLus(Box::new(gen), Box::new(pair))
                }
                Some((p, q)) => Hoon::Pair(
                    Box::new(grip(*car_skin, p, rel.clone())),
                    Box::new(grip(*cdr_skin, q, rel.clone())),
                ),
            }
        }

        Skin::Dbug(spot, inner_skin) => Hoon::Dbug(spot, Box::new(grip(*inner_skin, gen, rel))),

        Skin::Leaf(aura, atom) => Hoon::KetHep(Box::new(Spec::Leaf(aura, atom)), Box::new(gen)),

        Skin::Name(term, inner_skin) => Hoon::TisGal(
            Box::new(Hoon::Tune(TermOrTune::Term(term))),
            Box::new(grip(*inner_skin, gen, rel)),
        ),

        Skin::Over(mut wing, inner_skin) => {
            wing.extend(rel);
            grip(*inner_skin, gen, wing)
        }

        Skin::Spec(spec, inner_skin) => {
            let check_skin = if rel.is_empty() {
                spec
            } else {
                Box::new(Spec::Over(rel.clone(), spec))
            };

            let inner = grip(*inner_skin, gen, rel);

            Hoon::KetHep(check_skin, Box::new(inner))
        }

        Skin::Wash(depth) => {
            let wing: WingType = (0..depth).map(|_| Limb::Parent(0, None)).collect();
            Hoon::TisGal(Box::new(Hoon::Wing(wing)), Box::new(gen))
        }
    }
}

pub fn half(gen: Hoon) -> Option<(Hoon, Hoon)> {
    match gen {
        Hoon::Pair(car, cdr) => Some((*car, *cdr)),

        Hoon::Dbug(_spot, expr) => half(*expr),

        Hoon::ColCab(car, cdr) => Some((*cdr, *car)),

        Hoon::ColHep(car, cdr) => Some((*car, *cdr)),

        Hoon::ColKet(a, b, c, d) => {
            let tail = Hoon::ColLus(b, c, d);
            Some((*a, tail))
        }

        Hoon::ColSig(mut items) => {
            if items.is_empty() {
                None
            } else {
                let head = items.remove(0);
                Some((head, Hoon::ColSig(items)))
            }
        }

        Hoon::ColTar(mut items) => {
            if items.is_empty() {
                None
            } else if items.len() == 1 {
                half(items.remove(0))
            } else {
                let head = items.remove(0);
                let tail = Hoon::ColTar(items);
                Some((head, tail))
            }
        }

        _ => None,
    }
}

pub fn reek(gen: Hoon) -> Option<WingType> {
    match gen {
        Hoon::Pair(p, _q) => match *p {
            Hoon::Axis(a) => Some(vec![Limb::Axis(a)]),
            _ => None,
        },
        Hoon::Limb(t) => Some(vec![Limb::Term(t.clone())]),
        Hoon::Wing(w) => Some(w.to_vec()),
        Hoon::Dbug(_s, h) => reek(*h),
        _ => None,
    }
}

pub fn name_ax(gen: Hoon) -> Option<String> {
    match gen {
        Hoon::Wing(p) => {
            if p.is_empty() {
                None
            } else if let Some(i) = p.first() {
                match i {
                    Limb::Axis(_) => None,
                    Limb::Term(q) => Some(q.to_string()),
                    Limb::Parent(_, q) => q.clone(),
                }
            } else {
                None
            }
        }
        Hoon::Limb(p) => Some(p),
        Hoon::Dbug(_, q) => name_ax(*q),
        Hoon::TisGal(p, q) => name_ax(Hoon::TisGar(q, p)),
        Hoon::TisGar(_, q) => name_ax(*q),
        _ => None,
    }
}

pub fn autoname(mod_spec: Spec) -> Option<String> {
    //  ++autoname:ax
    match mod_spec {
        Spec::Base(base) => match base {
            BaseType::Atom(aura) => {
                if aura == "$" {
                    //  how empty terms will be represented here in rust land?...
                    Some("atom".to_string())
                } else {
                    Some(aura)
                }
            }
            _ => None,
        },
        Spec::Dbug(_, q) => autoname(*q),
        Spec::Leaf(p, _) => Some(p),
        Spec::Loop(p) => Some(p),
        Spec::Like(wing, _list_wing) => {
            if wing.is_empty() {
                None
            } else if let Some(i) = wing.first() {
                match i {
                    Limb::Axis(_) => None,
                    Limb::Term(q) => Some(q.to_string()),
                    Limb::Parent(_, q) => q.clone(),
                }
            } else {
                None
            }
        }
        Spec::Make(p, _) => name_ax(p),
        Spec::Made(_, q) => autoname(*q),
        Spec::Name(_, q) => autoname(*q),
        Spec::Over(_, q) => autoname(*q),
        Spec::BucBuc(p, _) => autoname(*p),
        Spec::BucBar(p, _) => autoname(*p),
        Spec::BucCab(p) => name_ax(p),
        Spec::BucCol(i, _) => autoname(*i),
        Spec::BucCen(i, _) => autoname(*i),
        Spec::BucDot(_, _) => None,
        Spec::BucGal(_, q) => autoname(*q),
        Spec::BucGar(_, q) => autoname(*q),
        Spec::BucHep(p, _) => autoname(*p),
        Spec::BucKet(_, q) => autoname(*q),
        Spec::BucLus(_, q) => autoname(*q),
        Spec::BucFas(_, _) => None,
        Spec::BucMic(p) => name_ax(p),
        Spec::BucPam(p, _) => autoname(*p),
        Spec::BucSig(_, q) => autoname(*q),
        Spec::BucTic(_, _) => None,
        Spec::BucTis(_, q) => autoname(*q),
        Spec::BucPat(_, q) => autoname(*q),
        Spec::BucWut(i, _) => autoname(*i),
        Spec::BucZap(_, _) => None,
    }
}

pub fn decorate(gen: Hoon, bug: Vec<Spot>, nut: Option<Note>) -> Hoon {
    let mut out = gen;

    for spot in bug.into_iter().rev() {
        out = Hoon::Dbug(spot, Box::new(out));
    }

    match nut {
        None => out,
        Some(note) => Hoon::Note(note, Box::new(out)),
    }
}

pub fn blue(tik: Tiki, gen: Hoon) -> Hoon {
    match tik {
        Tiki::Hoon((None, h)) => Hoon::TisGar(Box::new(Hoon::Axis(3)), Box::new(gen)),
        _ => gen,
    }
}

pub fn teal(tik: Tiki, mod_: Spec) -> Spec {
    match tik {
        Tiki::Wing((_, _)) => mod_,
        Tiki::Hoon((_, _)) => Spec::Over(vec![Limb::Axis(3)], Box::new(mod_)),
    }
}

pub fn tele(tik: Tiki, syn: Skin) -> Skin {
    match tik {
        Tiki::Wing((_, _)) => syn,
        Tiki::Hoon((_, _)) => Skin::Over(vec![Limb::Axis(3)], Box::new(syn)),
    }
}

pub fn gray(tik: Tiki, gen: Hoon) -> Hoon {
    match tik {
        Tiki::Wing((p, q)) => match p {
            None => gen,
            Some(u) => Hoon::TisTar((u, None), Box::new(Hoon::Wing(q)), Box::new(gen)),
        },
        Tiki::Hoon((p, q)) => {
            let arg = match p {
                None => q,
                Some(u) => Box::new(Hoon::KetTis(Skin::Term(u), q)),
            };
            Hoon::TisLus(arg, Box::new(gen))
        }
    }
}

pub fn puce(tik: Tiki) -> WingType {
    match tik {
        Tiki::Wing((p, q)) => match p {
            None => q,
            Some(u) => vec![Limb::Term(u)],
        },
        Tiki::Hoon((_, _)) => vec![Limb::Axis(2)],
    }
}

pub fn wthp(tik: Tiki, opt: Vec<(Spec, Hoon)>) -> Hoon {
    let mapped = opt
        .into_iter()
        .map(|(a, b)| (a, blue(tik.clone(), b)))
        .collect::<Vec<(Spec, Hoon)>>();
    gray(tik.clone(), Hoon::WutHep(puce(tik.clone()), mapped))
}

pub fn wtkt(tik: Tiki, sic: Hoon, non: Hoon) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutKet(
            puce(tik.clone()),
            Box::new(blue(tik.clone(), sic)),
            Box::new(blue(tik.clone(), non)),
        ),
    )
}

pub fn wtls(tik: Tiki, gen: Hoon, opt: Vec<(Spec, Hoon)>) -> Hoon {
    let mapped = opt
        .into_iter()
        .map(|(a, b)| (a, blue(tik.clone(), b)))
        .collect::<Vec<(Spec, Hoon)>>();
    gray(
        tik.clone(),
        Hoon::WutLus(puce(tik.clone()), Box::new(blue(tik.clone(), gen)), mapped),
    )
}

pub fn wtpt(tik: Tiki, sic: Hoon, non: Hoon) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutPat(
            puce(tik.clone()),
            Box::new(blue(tik.clone(), sic)),
            Box::new(blue(tik.clone(), non)),
        ),
    )
}

pub fn wtsg(tik: Tiki, sic: Hoon, non: Hoon) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutSig(
            puce(tik.clone()),
            Box::new(blue(tik.clone(), sic)),
            Box::new(blue(tik.clone(), non)),
        ),
    )
}

pub fn wthx(tik: Tiki, syn: Skin) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutHax(tele(tik.clone(), syn), puce(tik.clone())),
    )
}

pub fn wtts(tik: Tiki, mod_: Spec) -> Hoon {
    gray(
        tik.clone(),
        Hoon::WutTis(Box::new(teal(tik.clone(), mod_)), puce(tik.clone())),
    )
}

pub fn right_child(n: u64) -> u64 {
    if n == 0 {
        1
    } else {
        (2 * right_child(n - 1)) + 1
    }
}

pub fn left_child(n: u64) -> u64 {
    if n == 0 {
        0
    } else {
        2 * (left_child(n - 1) + 1)
    }
}

pub fn peg(a: u64, b: u64) -> Result<u64, &'static str> {
    if a == 0 || b == 0 {
        return Err("peg: a and b must be non-zero");
    }

    let k = b.ilog2();
    let offset = b & ((1u64 << k) - 1);
    Ok((a << k) + offset)
}

// non-control ASCII (32-255, excluding 127/DEL)
fn non_control_char<'src>() -> impl Parser<'src, &'src str, char, Err<'src>> {
    any()
        .filter(|c: &char| {
            let code = *c as u32;
            (code >= 0x20 && code < 0x7F) || code >= 0x80
        })
        .labelled("Non-Control Character")
}

fn gah<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    choice((just(' ').ignored(), newline())).labelled("Space or NewLine")
}

pub fn vul<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    just("::")
        .ignore_then(non_control_char().repeated())
        .ignore_then(newline())
        .ignored()
        .labelled("Comments")
}

fn gaq<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    choice((
        newline().ignored(),
        gah().ignore_then(choice((gah().ignored(), vul()))),
        vul(),
    ))
    .ignored()
    .labelled("End of Line")
}

pub fn gap<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    gaq()
        .then_ignore(choice((vul(), gah().ignored())).repeated().or_not())
        .ignored()
        .labelled("Gap")
}

pub fn list_term_hoon<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(String, Hoon)>, Err<'src>> {
    symbol()
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .repeated()
        .at_least(1)
        .collect::<Vec<(String, Hoon)>>()
}

pub fn list_names_tall<'src>() -> impl Parser<'src, &'src str, Vec<String>, Err<'src>> {
    symbol()
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .then_ignore(gap().ignore_then(just("==")))
}

pub fn list_names_wide<'src>() -> impl Parser<'src, &'src str, Vec<String>, Err<'src>> {
    symbol()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just("["), just("]"))
}

pub fn winglist<'src>() -> impl Parser<'src, &'src str, WingType, Err<'src>> {
    let name =      //  Name or $
        just('$')
            .to("$".to_string())
            .or(symbol());

    let com =   //  ,
        just(',')
        .to(Limb::Parent(0, None));

    let ket_name =   //  ^^name or name
        just('^')
            .repeated()
            .count()
            .then(name)
            .map(|(cnt, name)| {
                if cnt == 0 {
                    return Limb::Term(name);
                } else {
                    return Limb::Parent(cnt as u64, Some(name));
                }
            });

    let lus_number =   //  +10
            just('+')
                .ignore_then(digits())
                .map(|s| {
                    let num = s.parse::<u64>().unwrap();
                    Limb::Axis(num)
                });

    let pam_number =   //  &10
            just('&')
                .ignore_then(digits())
                .map(|s| {
                    let num = s.parse::<u64>().unwrap();
                    Limb::Axis(left_child(num))
                });

    let bar_number =  //  |10
           just('|').ignore_then(digits())
                .map(|s| {
                    let num = s.parse::<u64>().unwrap();
                    Limb::Axis(right_child(num))
                });

    let dot =  //  .
            just('.').to(Limb::Axis(1));

    let lus =  //  +
        just('+').to(Limb::Axis(3));

    let hep =  //  -
        just('-').to(Limb::Axis(2));

    let sign = any().filter(|c: &char| *c == '+' || *c == '-');
    let angle = any().filter(|c: &char| *c == '<' || *c == '>');

    let lark =   //    +>-<  notation
            sign
                .then(angle)
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>()
            .then(sign.or_not())
            .map(|(pairs, tail)| {
                let mut out = String::new();
                for (s, a) in pairs {
                    out.push(s);
                    out.push(a);
                }
                if let Some(t) = tail {
                    out.push(t);
                }
                out
            })
            .map(|s: String| {
                let mut axis = 1;
                for c in s.chars() {
                    match c {
                        '+' | '>' => axis = peg(axis, 3).unwrap(),
                        '-' | '<' => axis = peg(axis, 2).unwrap(),
                        _ => axis = 1,
                    }
                }
                Limb::Axis(axis)
            }).labelled("Lark Expression");

    choice((
        com, ket_name, lus_number, pam_number, bar_number, lark, dot, lus, hep,
    ))
    .separated_by(just('.'))
    .at_least(1)
    .collect::<Vec<_>>()
    .labelled("Wing")
}

pub fn variable_name_and_type<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Skin, Err<'src>> {
    let not_named = just('=') // =/  =foo
        .ignore_then(spec_wide.clone())
        .try_map(|spec, span| {
            let auto = autoname(spec.clone());
            match auto {
                None => Err(Rich::custom(span, "cannot autoname")),
                Some(term) => Ok(Skin::Name(
                    term,
                    Box::new(Skin::Spec(
                        Box::new(spec),
                        Box::new(Skin::Base(BaseType::NounExpr)),
                    )),
                )),
            }
        });

    let name_or_namedspec = symbol() //  =/  a=foo  ,  =/  a
        .then(
            just('/')
                .or(just('='))
                .ignore_then(spec_wide.clone())
                .or_not(),
        )
        .map(|(term, maybe_spec)| match maybe_spec {
            None => Skin::Term(term),
            Some(spec) => Skin::Name(
                term,
                Box::new(Skin::Spec(
                    Box::new(spec),
                    Box::new(Skin::Base(BaseType::NounExpr)),
                )),
            ),
        });

    let just_type = spec_wide
        .clone() // =/  type
        .map(|s| Skin::Spec(Box::new(s), Box::new(Skin::Base(BaseType::NounExpr))));

    choice((not_named, name_or_namedspec, just_type))
}

// ++  si                                                  ::  signed integer
pub fn syn_si(a: u128) -> bool {
    end_u128(0, 1, a) == 0
}

pub fn abs_si(a: u128) -> u128 {
    let rsh_res = rsh_u128(0, 1, a);
    let end_res = end_u128(0, 1, a.clone());
    end_res + rsh_res
}

pub fn old_si(a: u128) -> (bool, u128) {
    (syn_si(a), abs_si(a))
}
pub fn new_si(sign: bool, mag: u128) -> u128 {
    if mag == 0 {
        0
    } else if sign {
        mag << 1
    } else {
        (mag << 1) - 1
    }
}
fn sun_si(a: u128) -> u128 {
    a << 1
}

pub fn sum_si(a: u128, b: u128) -> u128 {
    let (c_sign, c_mag) = old_si(a);
    let (d_sign, d_mag) = old_si(b);
    match (c_sign, d_sign) {
        (false, false) => new_si(false, c_mag.wrapping_add(d_mag)),
        (false, true) => {
            if c_mag >= d_mag {
                new_si(false, c_mag - d_mag)
            } else {
                new_si(true, d_mag - c_mag)
            }
        }
        (true, false) => {
            if c_mag >= d_mag {
                new_si(true, c_mag - d_mag)
            } else {
                new_si(false, d_mag - c_mag)
            }
        }
        (true, true) => new_si(true, c_mag.wrapping_add(d_mag)),
    }
}

pub fn dif_si(a: u128, b: u128) -> u128 {
    let (b_sign, b_mag) = old_si(b);
    let neg_b = new_si(!b_sign, b_mag);
    sum_si(a, neg_b)
}

pub fn me(b: u128, p: u128) -> u128 {
    let t = dif_si(2, b);
    let p_si = sun_si(p);
    dif_si(t, p_si)
}

pub fn sig(p: usize, w: usize, a: &ParsedAtom) -> bool {
    let bit = cut(0, p + w, 1, a);
    match bit {
        ParsedAtom::Small(0) => true,
        ParsedAtom::Small(1) => false,
        _ => unreachable!(),
    }
}

pub fn sea(w: u128, p: u128, b: u128, a: &ParsedAtom) -> BinaryFloat {
    let f = cut(0, 0, p as usize, a);
    let e_atom = cut(0, p as usize, w as usize, a);
    let s = sig(p as usize, w as usize, a);

    let e = match e_atom {
        ParsedAtom::Small(x) => x,
        ParsedAtom::Big(_) => panic!("exponent field >128 bits"),
    };
    let f_u128 = match f {
        ParsedAtom::Small(x) => x,
        ParsedAtom::Big(_) => panic!("mantissa field >128 bits"),
    };

    let max_exp_field = sub_or_panic(bex(w), 1); // bex(w) >= 1

    if e == 0 {
        if f_u128 == 0 {
            BinaryFloat::Finite {
                sign: s,
                exp: 0,
                mant: BigUint::zero(),
            }
        } else {
            let me_val = me(b, p);
            BinaryFloat::Finite {
                sign: s,
                exp: me_val,
                mant: BigUint::from(f_u128),
            }
        }
    } else if e == max_exp_field {
        if f_u128 == 0 {
            BinaryFloat::Infinity { sign: s }
        } else {
            BinaryFloat::NaN
        }
    } else {
        let me_val = me(b, p);
        let q = sum_si(sum_si(sun_si(e), me_val), 1); // e + me + (-1)

        let r = f_u128.wrapping_add(bex(p));

        BinaryFloat::Finite {
            sign: s,
            exp: q,
            mant: BigUint::from(r),
        }
    }
}

//  inner function for drg_fl
pub fn drg(e: u128, a: BigUint, p: u128, v: u128, w: u128, d: char) -> (u128, BigUint) {
    assert!(!a.is_zero(), "drg: mantissa must be nonzero");
    println!("drg caleed e {} a {} p {} v {} w {} d {}", e, a, p, v, w, d);
    // drg caleed e 43 a 13176795 p 24 v 299 w 253 d d
    //  it should return (13, 31.415.927)
    //  but it returns 0 and 13176795

    let (e, a) = xpd(e, a, d, p, v);
    println!("xpd result: e:{} a:{}", e, a);
    assert!(!a.is_zero(), "xpd must not produce zero in drg");

    let (mut r, mut s, mut mn, mut mp) = {
        if syn_si(e) {
            let shift = abs_si(e) as usize;
            let r = lsh_big(0, shift, &a.clone());
            let s = BigUint::one();
            let mn = BigUint::one();
            let mp = BigUint::one();
            (r, s, mn, mp)
        } else {
            let shift = abs_si(e) as usize;
            let s = lsh_big(0, shift, &BigUint::one());
            let r = a.clone();
            let mn = BigUint::one();
            let mp = BigUint::one();
            (r, s, mn, mp)
        }
    };

    println!("r: {} s: {} mn: {} mp: {}", r, s, mn, mp);

    let a_orig = BigUint::from(1u128) << sub_or_panic(prc(p), 1); // 2^(p-1)
    let halfway = a == a_orig;
    let cond2 = e != v || d == 'i';
    if halfway && cond2 {
        r = lsh_big(0, 1, &r);
        s = lsh_big(0, 1, &s);
        mp = lsh_big(0, 1, &mp);
    }

    let mut k = 0u128; // --0 = 0 (@s zero)
    let ten = BigUint::from(10u32);
    let nine = BigUint::from(9u32);
    let q = (&s + &nine) / &ten;
    loop {
        if r >= q {
            break;
        }
        k = dif_si(k, 2);
        r *= &ten;
        mn *= &ten;
        mp *= &ten;
    }
    loop {
        let two_r = &r * 2u32;
        let left = &two_r + &mp;
        let right = &s * 2u32;
        if left < right {
            break;
        }
        s *= &ten;
        k = sum_si(k, 2);
    }

    let mut o = BigUint::zero();
    let mut u = BigUint::zero();

    loop {
        let (u_big, rem) = dvr_big(&(&r * &ten), &s);

        k = dif_si(k, 2);

        u = (u_big.to_u64().expect("digit ≥10") as u32).into();

        r = rem;
        mn *= &ten;
        mp *= &ten;

        let l = &r * 2u32 < mn;

        let two_s = &s * 2u32;
        let h = two_s < mp || (&r * 2u32 > sub_or_panic_big(&two_s, &mp));

        if !l && !h {
            o = o * &ten + u;
            continue;
        }

        let q = h && (!l || &r * 2u32 > s);
        let digit = if q { u + BigUint::one() } else { u };
        o = o * &ten + digit;
        break;
    }
    println!("drg returning {} {}", k, o);
    (k, o)
}

//  @rs to decimal float.
pub fn drg_fl(a: BinaryFloat, p: u128, w: u128, b: u128) -> DecimalFloat {
    match a {
        BinaryFloat::Finite { sign, exp, mant } => {
            if mant.is_zero() {
                DecimalFloat::Finite {
                    sign,
                    exp: 0,
                    mant: BigUint::zero(),
                }
            } else {
                let p = p + 1;
                let v = me(b, p);
                let w = bex(w) - 3;
                let d = 'd';
                let (k, digits) = drg(exp, mant, p, v, w, d);
                DecimalFloat::Finite {
                    sign,
                    exp: k,
                    mant: digits,
                }
            }
        }
        BinaryFloat::Infinity { sign } => DecimalFloat::Infinity { sign },
        BinaryFloat::NaN => DecimalFloat::NaN,
    }
}

// swr: swap rounding direction for negative numbers
pub fn swr(r: char) -> char {
    match r {
        'd' => 'u',
        'u' => 'd',
        _ => r,
    }
}

// fli: flip sign of BinaryFloat
pub fn fli(a: BinaryFloat) -> BinaryFloat {
    match a {
        BinaryFloat::Finite { sign, exp, mant } => BinaryFloat::Finite {
            sign: !sign,
            exp,
            mant,
        },
        BinaryFloat::Infinity { sign } => BinaryFloat::Infinity { sign: !sign },
        BinaryFloat::NaN => BinaryFloat::NaN,
    }
}

// zer: zero float node
pub fn zer() -> BinaryFloat {
    BinaryFloat::Finite {
        sign: false,
        exp: 0, // si-encoding of 0 is 0
        mant: BigUint::from(0u8),
    }
}

fn rau(e: u128, a: BigUint, t: bool, p: u128, v: u128, w: u128, r: char, d: char) -> BinaryFloat {
    let mode = match r {
        'z' | 'd' => LugMode::Floor,
        'a' | 'u' => LugMode::Ceiling,
        'n' => LugMode::Nearest,
        _ => LugMode::Nearest,
    };

    lug(mode, e, a, t, p, v, w, r, d)
}

pub fn cmp_si(a: u128, b: u128) -> u128 {
    if a == b {
        0
    } else if syn_si(a) {
        if syn_si(b) {
            if a > b {
                2
            } else {
                1
            }
        } else {
            2
        }
    } else if syn_si(b) {
        1
    } else {
        if a > b {
            1
        } else {
            2
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LugMode {
    Floor,   // %fl
    Ceiling, // %ce
    Smaller, // %sm
    Larger,  // %lg
    Nearest, // %ne  (ties to even)
    NearestAway,
    NearestTowards,
}

fn sub_or_panic(mut a: u128, b: u128) -> u128 {
    a = a.checked_sub(b).expect("subtraction underflow");
    a
}

fn sub_or_panic_big(a: &BigUint, b: &BigUint) -> BigUint {
    if a < b {
        panic!("subtraction underflow");
    }
    a - b
}

fn prc(p: u128) -> u128 {
    assert!(p > 1, "precision should be >= 2");
    p
}

fn lug(
    mode: LugMode,
    mut e: u128,
    mut a: BigUint,
    s: bool,
    p: u128,
    v: u128,
    w: u128,
    r: char,
    d: char,
) -> BinaryFloat {
    use BinaryFloat::*;
    use LugMode::*;

    if a == BigUint::zero() {
        panic!("lug: mantissa zero");
    }

    let m = met(0, &ParsedAtom::Big(a.clone())) as u128;
    let prc_res = prc(p);
    assert!(
        s | (m > prc_res),
        "lug: stick bit is false or precision is invalid"
    );

    let max_p = if m > prc_res {
        sub_or_panic(m as u128, prc_res)
    } else {
        0
    };

    let max_q = {
        let abs_arg = if d == 'i' {
            0
        } else if cmp_si(e, v) == 1 {
            dif_si(v, e)
        } else {
            0
        };
        abs_si(abs_arg)
    };

    let q = max_p.max(max_q);

    let b = end_big(0, q as usize, &a)
        .to_u128()
        .expect("value too large for u128");

    a = rsh(0, q as usize, &ParsedAtom::Big(a)).to_biguint();

    e = sum_si(e, sun_si(q));

    if a == BigUint::zero() {
        assert!(d != 'i', "lug: d == %i");
        return match mode {
            Floor | Smaller => Finite {
                sign: true,
                exp: 0,
                mant: BigUint::zero(),
            },
            Ceiling | Larger => Finite {
                sign: true,
                exp: v,
                mant: BigUint::one(),
            },
            Nearest | NearestTowards => {
                let half = bex(q.saturating_sub(1));
                if s {
                    if b <= half {
                        return Finite {
                            sign: true,
                            exp: 0,
                            mant: BigUint::zero(),
                        };
                    }
                    return Finite {
                        sign: true,
                        exp: v,
                        mant: BigUint::one(),
                    };
                }
                if b < half {
                    return Finite {
                        sign: true,
                        exp: 0,
                        mant: BigUint::zero(),
                    };
                }
                return Finite {
                    sign: true,
                    exp: v,
                    mant: BigUint::one(),
                };
            }
            NearestAway => {
                let half = bex(q.saturating_sub(1));
                if b < half {
                    return Finite {
                        sign: true,
                        exp: 0,
                        mant: BigUint::zero(),
                    };
                }
                return Finite {
                    sign: true,
                    exp: v,
                    mant: BigUint::one(),
                };
            }
        };
    }

    (e, a) = xpd(e, a, d, p, v);

    match mode {
        Floor => { /* no change */ }
        Larger => a = a + BigUint::one(),
        Smaller => {
            if b == 0 && s {
                if e == v && d != 'i' {
                    a = sub_or_panic_big(&a, &BigUint::one());
                } else {
                    let y =
                        sub_or_panic_big(&(a.clone() * BigUint::from(2 as u128)), &BigUint::one());
                    if met_big(0, &y) as u128 <= prc_res {
                        a = y;
                        e = dif_si(e, 2);
                    } else {
                        a = sub_or_panic_big(&a, &BigUint::one());
                    }
                }
            }
        }
        Ceiling => {
            if !(b == 0 && !s) {
                a = a + BigUint::one();
            }
        }
        Nearest => {
            if b != 0 {
                let y = bex(sub_or_panic(q, 1));
                if b == y && s {
                    if dis_big(&a, &BigUint::one()) != BigUint::zero() {
                        a = a + BigUint::one();
                    }
                } else if b < y {
                } else {
                    a = a + BigUint::one();
                }
            }
        }
        NearestAway => {
            if b != 0 {
                let y = bex(sub_or_panic(q, 1));
                if !(b < y) {
                    a = a + BigUint::one();
                }
            }
        }
        NearestTowards => {
            if b != 0 {
                let y = bex(sub_or_panic(q, 1));
                if b == y {
                    if !s {
                        a = a + BigUint::one();
                    }
                }
                if !(b < y) {
                    a = a + BigUint::one();
                }
            }
        }
    };

    (e, a) = if (met_big(0, &a.clone()) as u128) != (prc_res + 1) {
        (e, a)
    } else {
        a = rsh(0, 1, &ParsedAtom::Big(a))
            .to_u128()
            .expect("lug: cast failled")
            .into();
        e = sum_si(e, 2);
        (e, a)
    };

    if a == BigUint::zero() {
        return Finite {
            sign: true,
            exp: 0,
            mant: BigUint::zero(),
        };
    }

    let res = if d == 'i' {
        Finite {
            sign: true,
            exp: e,
            mant: BigUint::from(a),
        }
    } else if cmp_si(emx(v, w), e) == 1 {
        Infinity { sign: true }
    } else {
        Finite {
            sign: true,
            exp: e,
            mant: BigUint::from(a),
        }
    };

    if !(d == 'f') {
        return res;
    }

    match res {
        Finite {
            sign,
            exp,
            ref mant,
        } => {
            if met_big(0, &mant.clone()) as u128 == prc(p) {
                return Finite {
                    sign: true,
                    exp: 0,
                    mant: BigUint::zero(),
                };
            }
            res
        }
        _ => res,
    }
}

fn emx(v: u128, w: u128) -> u128 {
    sum_si(v, sun_si(w))
}

fn rou(e: u128, a: BigUint, p: u128, v: u128, w: u128, r: char, d: char) -> BinaryFloat {
    rau(e, a, true, p, v, w, r, d)
}

pub fn binaryfloat_mul_internal(
    a_e: u128,
    a_a: BigUint,
    b_e: u128,
    b_a: BigUint,
    p: u128,
    v: u128,
    w: u128,
    r: char,
    d: char,
) -> BinaryFloat {
    let e = sum_si(a_e, b_e);
    let a = a_a * b_a;
    rou(e, a, p, v, w, r, d)
}

pub fn binaryfloat_div_internal(
    a_e: u128,
    a_a: BigUint,
    b_e: u128,
    b_a: BigUint,
    p: u128,
    v_min: u128,
    w: u128,
    r: char,
    d: char,
) -> BinaryFloat {
    let ma = met_big(0, &a_a) as u128;
    let mb = met_big(0, &b_a) as u128;

    let rhs = sun_si(mb + prc(p) + 1);
    let v = dif_si(sun_si(ma), rhs);

    let (a_e_shifted, a_a_shifted) = if syn_si(v) {
        (a_e, a_a)
    } else {
        let shift = abs_si(v) as usize;
        let new_e = sum_si(v, a_e);
        let new_a = lsh(0, shift, &ParsedAtom::Big(a_a.clone())).to_biguint();
        (new_e, new_a)
    };

    let j = dif_si(a_e_shifted, b_e);
    let (quot, rem) = dvr_big(&a_a_shifted, &b_a);

    rau(j, quot, rem.is_zero(), p, v_min, w, r, d)
}

fn dvr_big(a: &BigUint, b: &BigUint) -> (BigUint, BigUint) {
    let quot = a / b;
    let rem = a % b;
    (quot, rem)
}

pub fn bex(a: u128) -> u128 {
    if a == 0 {
        1
    } else {
        assert!(a < 128, "bex: exponent too large for u128");
        1u128 << a
    }
}

fn xpd(e: u128, a: BigUint, d: char, p: u128, v: u128) -> (u128, BigUint) {
    let ma = met_big(0, &a.clone()) as u128;

    if ma >= prc(p) {
        return (e, a);
    }

    let shift = if d == 'i' {
        sub_or_panic(prc(p), ma as u128)
    } else {
        let w = dif_si(e, v);
        let q = if syn_si(w) { abs_si(w) } else { 0 };
        let needed = sub_or_panic(prc(p), ma as u128);
        q.min(needed)
    };

    let e_new = dif_si(e, sun_si(shift));
    let a_new = lsh_big(0, shift as usize, &a);

    (e_new, a_new)
}

pub fn binaryfloat_mul(
    a: BinaryFloat,
    b: BinaryFloat,
    p: u128,
    v: u128,
    w: u128,
    mut r: char,
    d: char,
) -> BinaryFloat {
    use BinaryFloat::*;

    if matches!(a, NaN) || matches!(b, NaN) {
        return NaN;
    }

    if let Infinity { sign: sa } = a {
        if let Infinity { sign: sb } = b {
            return Infinity { sign: sa == sb };
        }

        let b_mant = if let Finite { ref mant, .. } = b {
            mant.clone()
        } else {
            BigUint::zero()
        };
        if b_mant == BigUint::zero() {
            return NaN;
        }
        return Infinity {
            sign: sa == b.sign(),
        };
    }

    if let Infinity { sign: sb } = b {
        let a_mant = if let Finite { ref mant, .. } = a {
            mant.clone()
        } else {
            BigUint::zero()
        };
        if a_mant == BigUint::zero() {
            return NaN;
        }
        return Infinity {
            sign: a.sign() == sb,
        };
    }

    let (sa, ea, ma) = if let Finite { sign, exp, mant } = a {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };
    let (sb, eb, mb) = if let Finite { sign, exp, mant } = b {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };

    if ma == BigUint::zero() || mb == BigUint::zero() {
        return Finite {
            sign: sa == sb, // =(s.a s.b)
            exp: 0,         // zer = [e=--0 a=0]
            mant: BigUint::zero(),
        };
    }

    if ma == BigUint::zero() || mb == BigUint::zero() {
        return binaryfloat_mul_internal(ea, ma, eb, mb, p, v, w, r, d);
    }
    r = swr(r);
    fli(binaryfloat_mul_internal(ea, ma, eb, mb, p, v, w, r, d))
}

pub fn binaryfloat_div(
    a: BinaryFloat,
    b: BinaryFloat,
    p: u128,
    v: u128,
    w: u128,
    mut r: char,
    d: char,
) -> BinaryFloat {
    use BinaryFloat::*;

    if matches!(a, NaN) || matches!(b, NaN) {
        return NaN;
    }

    if let Infinity { sign: sa } = a {
        if let Infinity { sign: sb } = b {
            return NaN;
        }
        return Infinity {
            sign: sa == b.sign(),
        };
    }

    if let Infinity { sign: sb } = b {
        return Finite {
            sign: a.sign() == sb,
            exp: 0, // zer = [e=--0 a=0]
            mant: BigUint::zero(),
        };
    }

    let (sa, ea, ma) = if let Finite { sign, exp, mant } = a {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };
    let (sb, eb, mb) = if let Finite { sign, exp, mant } = b {
        (sign, exp, mant)
    } else {
        (false, 0, BigUint::zero())
    };

    if ma == BigUint::zero() {
        if mb == BigUint::zero() {
            return NaN;
        }
        return Finite {
            sign: sa == sb,
            exp: 0,
            mant: BigUint::zero(),
        };
    }

    if mb == BigUint::zero() {
        return Infinity { sign: sa == sb };
    }

    if sa == sb {
        return binaryfloat_div_internal(ea, ma, eb, mb, p, v, w, r, d);
    }
    r = swr(r);
    fli(binaryfloat_div_internal(ea, ma, eb, mb, p, v, w, r, d))
}

pub fn pow(base: u128, exp: u128) -> BigUint {
    if exp == 0 {
        return BigUint::from(1u8);
    }

    let mut result = BigUint::from(1u8);
    let mut base = BigUint::from(base);
    let mut exp = exp;

    while exp > 0 {
        if exp & 1 == 1 {
            result *= &base;
        }
        base *= base.clone();
        exp >>= 1;
    }

    result
}

pub fn fil(a: u32, b: u32, c: u128) -> ParsedAtom {
    if b == 0 {
        return ParsedAtom::Small(0);
    }

    let bloq_bits = 1u32 << a; // 2^a bits per block
    let mask = if bloq_bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bloq_bits) - 1
    };
    let c_masked = c & mask;

    if bloq_bits as u64 * b as u64 <= 128 && c_masked != 0 {
        let mut result = 0u128;
        for i in 0..b {
            let shift = (b - 1 - i) as u32 * bloq_bits;
            if shift >= 128 {
                break;
            }
            result |= c_masked << shift;
        }
        ParsedAtom::Small(result)
    } else {
        let c_big = BigUint::from(c_masked);
        let mut result = BigUint::from(0u8);
        for i in 0..b {
            let shift = (b - 1 - i) as usize * bloq_bits as usize;
            result |= &c_big << shift;
        }
        ParsedAtom::Big(result)
    }
}

pub fn bif(a: BinaryFloat, w: u128, p: u128, b: u128, r: char) -> ParsedAtom {
    match a {
        BinaryFloat::Infinity { sign } => {
            let fill_val = fil(0, w as u32, 1);
            let q = lsh(0, p as usize, &fill_val);
            if sign {
                q
            } else {
                let q_u128 = q.to_u128().expect("float bigger than 128 bits");
                ParsedAtom::Small(q_u128.wrapping_add(bex(w + p)))
            }
        }

        BinaryFloat::NaN => {
            let fill_val = fil(0, (w + 1) as u32, 1);
            let shift = sub_or_panic(p, 1) as usize;
            if shift >= 128 {
                panic!("bif: shift too large");
            }
            lsh(0, shift, &fill_val)
        }

        BinaryFloat::Finite {
            sign,
            exp: e,
            mant: a_a,
        } => {
            if a_a.is_zero() {
                return if sign {
                    ParsedAtom::Small(0)
                } else {
                    ParsedAtom::Small(bex(w + p))
                };
            }

            let ma = met_big(0, &a_a) as u128;

            if ma != p + 1 {
                assert!(
                    e == dif_si(dif_si(2, b), sun_si(p)),
                    "bif: subnormal exponent != me"
                );
                assert!(ma < p + 1, "bif: subnormal mantissa too large");

                let a_small = if a_a.bits() > 128 {
                    panic!("bif: mantissa too large for Small");
                } else {
                    a_a.to_u128().unwrap()
                };

                return if sign {
                    ParsedAtom::Small(a_small)
                } else {
                    ParsedAtom::Small(a_small.wrapping_add(bex(w + p)))
                };
            }

            let diff = dif_si(e, dif_si(dif_si(2, b), sun_si(p)));
            let q = sum_si(diff, 2);

            let abs_q = abs_si(q);
            let shifted = (abs_q as u128) << p;
            let a_small = if a_a.bits() > 128 {
                panic!("bif: mantissa too large");
            } else {
                a_a.to_u128().unwrap()
            };
            let low_p = a_small & ((1u128 << p) - 1);
            let r = shifted.wrapping_add(low_p);

            if sign {
                ParsedAtom::Small(r)
            } else {
                ParsedAtom::Small(r.wrapping_add(bex(w + p)))
            }
        }
    }
}

pub fn grd_fl(a: DecimalFloat, b: u128, p: u128, w: u128, mut r: char) -> BinaryFloat {
    //  +pa:ff arm will set these configs before calling +grd:fl
    let v = me(b, p);
    let p = p + 1;
    let w = bex(w) - 3;
    let d = 'd';

    match a {
        DecimalFloat::NaN => BinaryFloat::NaN,
        DecimalFloat::Infinity { sign } => BinaryFloat::Infinity { sign },
        DecimalFloat::Finite { sign, exp: e, mant } => {
            r = 'n';
            let q = abs_si(e);
            let pow5 = pow(5, q);

            let left = BinaryFloat::Finite {
                sign,
                exp: 0,
                mant: BigUint::from(mant),
            };
            if syn_si(e) {
                let right = BinaryFloat::Finite {
                    sign: true,
                    exp: e,
                    mant: pow5,
                };
                binaryfloat_mul(left, right, p, v, w, r, d)
            } else {
                let divisor = BinaryFloat::Finite {
                    sign: true,
                    exp: sun_si(q),
                    mant: pow5,
                };
                binaryfloat_div(left.clone(), divisor.clone(), p, v, w, r, d)
            }
        }
    }
}

//  finish parsing @rh
//  rylh -> grd:rh -> grd:ff -> grd:fl
pub fn rylh(a: DecimalFloat) -> ParsedAtom {
    let w = 5;
    let p = 10;
    let b = 30; // --15
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

//  prep @rh for print
pub fn rlyh(a: u128) -> DecimalFloat {
    let w = 5;
    let p = 10;
    let b = 30; // --15
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

//  finish parsing @rq
pub fn rylq(a: DecimalFloat) -> ParsedAtom {
    let w = 15;
    let p = 112;
    let b = 32766; // --16.383
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

//  prep @rq for print
pub fn rlyq(a: u128) -> DecimalFloat {
    let w = 15;
    let p = 112;
    let b = 32766; // --16.383
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

//  finish parsing @rd
pub fn ryld(a: DecimalFloat) -> ParsedAtom {
    let w = 11;
    let p = 52;
    let b = 2046; // --1.023
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

//  prep @rd for print
pub fn rlyd(a: u128) -> DecimalFloat {
    let w = 11;
    let p = 52;
    let b = 2046; // --1.023
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

//  finish parsing @rs
pub fn ryls(a: DecimalFloat) -> ParsedAtom {
    let w = 8;
    let p = 23;
    let b = 254; // --127
    let r = 'z';
    let grd_res = grd_fl(a, b, p, w, r);
    bif(grd_res, w, p, b, r)
}

// prep @rs for print
pub fn rlys(a: u128) -> DecimalFloat {
    let w = 8;
    let p = 23;
    let b = 254; // --127
    let r = 'z';
    let sea_res = sea(w, p, b, &ParsedAtom::Small(a));
    drg_fl(sea_res, p, w, b)
}

pub fn float<'src>() -> impl Parser<'src, &'src str, (String, ParsedAtom), Err<'src>> {
    let floats = just('-')
        .or_not()
        .then(decimal_without_leading_zero())
        .then(choice((
            just('.').ignore_then(digits()).map(|frac| {
                (
                    frac.len(),
                    frac.parse::<BigUint>().expect("float: invalid fraction"),
                )
            }),
            empty().to((0, BigUint::zero())),
        )))
        .then(choice((
            just('e')
                .ignore_then(just('-').or_not())
                .then(decimal_without_leading_zero())
                .map(|(maybe_hep, expo)| {
                    (
                        !maybe_hep.is_some(),
                        expo.parse::<u128>().expect("float: invalid exponent"),
                    )
                }),
            empty().to((true, 0)),
        )))
        .map(|(((maybe_hep, p), (len_mant, mant)), (sign_expo, expo))| {
            let term1 = new_si(sign_expo, expo);
            let term2 = sun_si(len_mant as u128);
            let h = dif_si(term1, term2);
            let po = BigUint::from(10u32).pow(len_mant.try_into().unwrap());
            let integer_part = p.parse::<BigUint>().expect("float: invalid decimal");
            let a = integer_part * po + mant;
            DecimalFloat::Finite {
                sign: !maybe_hep.is_some(),
                exp: h,
                mant: a,
            }
        });

    let inf = just('-')
        .or_not() //  -inf or inf
        .then(just("inf"))
        .map(|(maybe_hep, inf)| DecimalFloat::Infinity {
            sign: !maybe_hep.is_some(),
        })
        .boxed();

    let nan = just("nan").to(DecimalFloat::NaN).boxed(); //  nan

    let royl_rn = choice((
        floats, ///  1.10 or 1e10
        inf, nan,
    ))
    .boxed();

    let rh = just("~~").ignore_then(royl_rn.clone());
    let rq = just("~~~").ignore_then(royl_rn.clone());
    let rd = just('~').ignore_then(royl_rn.clone());
    let rs = royl_rn;

    choice((
        rh.map(|dn| ("rh".to_string(), rylh(dn))),
        rq.map(|dn| ("rq".to_string(), rylq(dn))),
        rd.map(|dn| ("rd".to_string(), ryld(dn))),
        rs.map(|dn| ("rs".to_string(), ryls(dn))),
    ))
    .labelled("Float")
}

pub fn list_wing_hoon_wide<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(WingType, Hoon)>, Err<'src>> {
    let pair = winglist().then_ignore(just(' ')).then(hoon.clone());

    pair.separated_by(just(",").then(just(' ')))
        .at_least(1)
        .collect::<Vec<_>>()
}

pub fn list_hoon_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<Hoon>, Err<'src>> {
    hoon_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<Hoon>>()
}

pub fn list_spec_closed_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Vec<Spec>, Err<'src>> {
    spec_wide
        .clone()
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('('), just(')'))
}

pub fn list_spec_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Vec<Spec>, Err<'src>> {
    gap()
        .ignore_then(
            spec.clone()
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("=="))
}

pub fn list_wing_hoon_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(WingType, Hoon)>, Err<'src>> {
    let pair = winglist()
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap());

    pair.repeated()
        .at_least(1)
        .collect::<Vec<(WingType, Hoon)>>()
}

pub fn tiki_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Tiki, Err<'src>> {
    let with_name = symbol()
        .then_ignore(just('='))
        .then(
            winglist()
                .map(|w| {
                    Box::new(move |t: String| Tiki::Wing((Some(t), w)))
                        as Box<dyn FnOnce(String) -> Tiki>
                })
                .or(hoon_wide.clone().map(|h| {
                    Box::new(move |t: String| Tiki::Hoon((Some(t), Box::new(h))))
                        as Box<dyn FnOnce(String) -> Tiki>
                })),
        )
        .map(|(t, f)| f(t));

    let no_name = winglist()
        .map(|w| Tiki::Wing((None, w)))
        .or(hoon_wide.clone().map(|h| Tiki::Hoon((None, Box::new(h)))));

    with_name.or(no_name)
}

pub fn tiki_tall<'src>(
    hoon_tall: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Tiki, Err<'src>> {
    let with_name = symbol()
        .then_ignore(just('='))
        .then(
            winglist()
                .map(|w| {
                    Box::new(move |t: String| Tiki::Wing((Some(t), w)))
                        as Box<dyn FnOnce(String) -> Tiki>
                })
                .or(hoon_tall.clone().map(|h| {
                    Box::new(move |t: String| Tiki::Hoon((Some(t), Box::new(h))))
                        as Box<dyn FnOnce(String) -> Tiki>
                })),
        )
        .map(|(t, f)| f(t));

    tiki_wide(hoon_wide.clone()) //  the hoon parser has ^= case here but
        .or(just("^=").then(gap()).or_not().ignore_then(with_name))
        .or(hoon_tall.clone().map(|h| Tiki::Hoon((None, Box::new(h)))))
}

///  Parses arms of a Core (grouped by chapters).
///     chapters can be unamed or named with +$
///     arms can be named with ++ or +$
///
pub fn chapters<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, HashMap<String, Tome>, Err<'src>> {
    let luslus = just("++")
        .ignore_then(gap())
        .ignore_then(just('$').to("$".to_string()).or(symbol()))
        .then_ignore(gap())
        .then(hoon.clone())
        .map(|(name, hoon)| (name, hoon))
        .labelled("Arm ++");

    let lusbuc = just("+$")
        .ignore_then(gap())
        .ignore_then(symbol())
        .then_ignore(gap())
        .then(spec.clone())
        .map(|(name, spec)| {
            (
                name.clone(),
                Hoon::KetCol(Box::new(Spec::Name(name.clone(), Box::new(spec)))),
            )
        })
        .labelled("Arm +$");

    let optional_chapter_label = just("+|")
        .then_ignore(gap())
        .then(just("%"))
        .ignore_then(symbol())
        .then_ignore(gap())
        .or_not()
        .labelled("Chapter Label +|");

    let chapter = optional_chapter_label.then(
        luslus
            .or(lusbuc)
            .then_ignore(gap())
            .repeated()
            .at_least(1)
            .collect::<Vec<_>>(),
    );

    chapter
        .repeated()
        .at_least(1)
        .collect::<Vec<_>>()
        .then_ignore(just("--"))
        .map(|chapters_vec: Vec<(Option<String>, Vec<(String, Hoon)>)>| {
            let mut map_term_tome = HashMap::new();
            for (opt_label, arms_vec) in chapters_vec {
                let mut arms_map = HashMap::new();
                for (name, hoon) in arms_vec {
                    arms_map.insert(name, hoon);
                }
                let key = opt_label.unwrap_or_else(|| "$".to_string());
                map_term_tome.insert(key, (None, arms_map));
            }
            map_term_tome
        })
}

pub fn list_hoon_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<Hoon>, Err<'src>> {
    hoon.clone()
        .separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
}

pub fn term<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just("%").ignore_then(symbol())
}

pub fn jet_hooks<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Vec<(String, Hoon)>, Err<'src>> {
    just('~').to(Vec::new()).or(just("==")
        .ignore_then(gap())
        .ignore_then(
            just("%")
                .ignore_then(symbol())
                .then_ignore(gap())
                .then(hoon.clone())
                .separated_by(gap())
                .at_least(1)
                .collect::<Vec<(String, Hoon)>>(),
        )
        .then_ignore(gap())
        .then_ignore(just("==")))
}

pub fn jet_signature<'src>() -> impl Parser<'src, &'src str, Chum, Err<'src>> {
    let lef = symbol().map(Chum::Lef); //  %k

    let stdkel = symbol() //  %k.138
        .then_ignore(just('.'))
        .then(decimal_number())
        .map(|(s, n)| Chum::StdKel(s, decimal_to_atom(n)));

    let venprokel = symbol() //  %k:foo.138
        .then_ignore(just(':'))
        .then(symbol())
        .then_ignore(just('.'))
        .then(decimal_number())
        .map(|((s1, s2), n)| Chum::VenProKel(s1, s2, decimal_to_atom(n)));

    let venproverkel =  //  %k:foo:bar..138
                symbol()
                .then_ignore(just(':'))
                .then(symbol())
                .then_ignore(just(".."))
                .then(decimal_number())
                .map(|((s1, s2), n)| Chum::VenProKel(s1, s2, decimal_to_atom(n)));

    just("%")
        .ignore_then(choice((venproverkel, venprokel, stdkel, lef)))
        .labelled("Jet Signature")
}

//  +lute
//
pub fn noun_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon.separated_by(gap())
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('[').ignore_then(gap()), gap().ignore_then(just(']')))
        .map(|h| Hoon::ColTar(h))
}

pub fn newline<'src>() -> impl Parser<'src, &'src str, (), Err<'src>> {
    just('\n').labelled("Newline").ignored()
}

pub fn soil<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Vec<Woof>, Err<'src>> {
    let sump = hoon_wide
        .separated_by(just(' '))
        .at_least(1)
        .collect::<Vec<_>>()
        .delimited_by(just('{'), just('}'))
        .map(|h| Woof::Hoon(Hoon::ColTar(h)))
        .boxed();

    // non-control 32-256, excluding DEL, {,  ", \
    let wide_char = any().filter(|c: &char| {
        let x = *c as u32;
        (x >= 0x20 && x <= 0x7E && *c != '{' && *c != '"' && *c != '\\') || (x >= 0x80 && x <= 0xFF)
    });

    //
    //  "foo"
    //
    let wide_tape = choice((
        //
        //  escaped \, ", {, hex
        //
        just("\\")
            .ignore_then(choice((
                just("\\").to('\\'),
                just("\"").to('\"'),
                just("{").to('{'),
                // \HH hex escape
                any()
                    .filter(|c: &char| c.is_ascii_hexdigit())
                    .then(any().filter(|c: &char| c.is_ascii_hexdigit()))
                    .map(|(a, b)| {
                        let hx = format!("{}{}", a, b);
                        let byte = u8::from_str_radix(&hx, 16).unwrap();
                        byte as char
                    }),
            )))
            .map(|c: char| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
        //
        //  {hoon}
        //
        sump.clone(),
        ///
        wide_char.map(|c| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
    ))
    .repeated()
    .collect::<Vec<Woof>>()
    .delimited_by(just("\""), just("\""))
    .labelled("Tape");

    // non-control 32-256, excluding DEL, {,  \
    let tall_char = any().filter(|c: &char| {
        let x = *c as u32;
        (x >= 0x20 && x <= 0x7E && *c != '{' && *c != '\\') || (x >= 0x80 && x <= 0xFF)
    });

    // let tall_tape_line_break =
    //             newline()
    //             .ignore_then(just("\"\"\"").not())
    //             .to(Woof::ParsedAtom(ParsedAtom::Small('\n' as u128)));

    let tall_tape_line_content = choice((
        //
        //  escaped \, {, hex
        //
        just("\\")
            .ignore_then(choice((
                just("\\").to('\\'),
                just("{").to('{'),
                // \HH hex escape
                any()
                    .filter(|c: &char| c.is_ascii_hexdigit())
                    .then(any().filter(|c: &char| c.is_ascii_hexdigit()))
                    .map(|(a, b)| {
                        let hx = format!("{}{}", a, b);
                        let byte = u8::from_str_radix(&hx, 16).unwrap();
                        byte as char
                    }),
            )))
            .map(|c: char| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
        //
        tall_char.map(|c| Woof::ParsedAtom(ParsedAtom::Small(c as u128))),
        //
        //  {hoon}
        //
        sump,
    ))
    .repeated()
    .collect::<Vec<Woof>>();

    let prefix_spaces = just(' ').repeated();

    let tall_tape_open = just("\"\"\"").map_with(move |_, extra| {
        let span: SimpleSpan = extra.span(); // get identation
        let (_line, col) = linemap.line_col(span.start);
        if col != 0 {
            return (col - 1) as usize;
        }
        return 0 as usize;
    });

    let tall_tape_close = newline()
        .ignore_then(just(' ').repeated().count())
        .then_ignore(just("\"\"\""))
        .boxed();

    let tall_tape_line = tall_tape_close.clone().not().ignore_then(
        newline()
            .ignore_then(just(' ').repeated().count())
            .then(tall_tape_line_content),
    );

    //  """
    //  foo
    //  """
    let tall_tape = prefix_spaces
        .ignore_then(tall_tape_open)
        .then(tall_tape_line.repeated().collect::<Vec<_>>())
        .then(tall_tape_close)
        .validate(|((absolute_indent, lines), close_indent), extra, emit| {
            let span = extra.span();

            if close_indent != absolute_indent {
                emit.emit(Rich::custom(span, "closing delimiter indentation mismatch"));
                return Vec::new();
            }

            let mut out: Vec<Woof> = vec![];
            for (mut indent, mut line) in lines {
                if indent > absolute_indent {
                    let extra = indent - absolute_indent;
                    indent = absolute_indent;
                    //  extra whitespaces belongs longs to line not indentation
                    let space = Woof::ParsedAtom(ParsedAtom::Small(' ' as u128));
                    line.splice(0..0, std::iter::repeat(space).take(extra));
                }

                //  if line is just a linebreak allow it
                if indent != absolute_indent && !(line.is_empty() && (indent == 0 as usize)) {
                    emit.emit(Rich::custom(span, "inconsistent indentation in tall tape"));
                    return Vec::new();
                }
                out.push(Woof::ParsedAtom(ParsedAtom::Small('\n' as u128)));
                if !line.is_empty() {
                    out.extend(line);
                }
            }
            // first linebreak after """ should not be in the tape
            out.remove(0);
            out
        })
        .labelled("Tape");

    choice((tall_tape, wide_tape))
}

pub fn tape<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    soil(hoon_wide.clone(), linemap.clone())
        .separated_by(just('.').ignore_then(gap().or_not()))
        .at_least(1)
        .collect::<Vec<_>>()
        .map(|s: Vec<Vec<Woof>>| {
            let wof: Vec<Woof> = s.into_iter().flatten().collect();
            Hoon::Knit(wof)
        })
        .labelled("Tape")
}

pub fn aura_text<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just('@')
        .ignore_then(
            any()
                .filter(|c: &char| c.is_ascii_lowercase())
                .repeated()
                .collect::<Vec<char>>()
                .then(
                    any()
                        .filter(|c: &char| c.is_ascii_uppercase())
                        .repeated()
                        .collect::<Vec<char>>(),
                )
                .map(|(lowers, uppers)| {
                    let mut s = String::new();
                    s.extend(lowers);
                    s.extend(uppers);
                    s
                }),
        )
        .labelled("Aura<@foo>")
}

pub fn aura_hoon<'src>() -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    aura_text()
        .map(|s| Hoon::Base(BaseType::Atom(s)))
        .labelled("Aura")
}

pub fn aura_spec<'src>() -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    aura_text()
        .map(|s| Spec::Base(BaseType::Atom(s)))
        .labelled("Aura")
}

pub fn loop_spec<'src>() -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    just('/')
        .ignore_then(choice((just('$').to("$".to_string()), symbol())))
        .map(|s| Spec::Loop(s))
}

pub fn concatanate<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just('^'))
        .then(hoon_wide.clone())
        .map(|(p, q)| Hoon::Pair(Box::new(p), Box::new(q)))
}

pub fn wing<'src>() -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    winglist()
        .map(|list: WingType| match list.first() {
            Some(Limb::Axis(0)) | Some(Limb::Term(_)) | Some(Limb::Parent(_, _)) => {
                Hoon::Wing(list)
            }
            _ => Hoon::CenTis(list, vec![]),
        })
        .labelled("Wing")
}

pub fn tell<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just("<")
        .ignore_then(list_hoon_wide(hoon_wide.clone()))
        .then_ignore(just(">"))
        .map(|list| Hoon::Tell(list))
}

pub fn yell_parser<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just(">")
        .ignore_then(list_hoon_wide(hoon_wide.clone()))
        .then_ignore(just("<"))
        .map(|list| Hoon::Yell(list))
}

pub fn constant<'src>(linemap: Arc<LineMap>) -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    let buc =      // %$
        just('$')
        .to(Coin::Dime("tas".to_string(), ParsedAtom::Small(0)));

    let cord =      // %'foo'
        cord(linemap)
        .map(|s| Coin::Dime("t".to_string(), s));

    let coin =      // %123, %~m5, etc.
        nuck();

    let no = just('|').to(Coin::Dime("f".to_string(), ParsedAtom::Small(1)));

    let yes = just('&').to(Coin::Dime("f".to_string(), ParsedAtom::Small(0)));

    just('%')
        .ignore_then(choice((buc, yes, no, cord, coin)))
        .labelled("Constant<%foo>")
}

pub fn cord<'src>(linemap: Arc<LineMap>) -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    //  \\, \' and \AA were A is a hex digit
    let escape = just('\\').ignore_then(choice((
        just('\\').to('\\'),
        just('\'').to('\''),
        // \HH hex escape
        any()
            .filter(|c: &char| c.is_ascii_hexdigit())
            .then(any().filter(|c: &char| c.is_ascii_hexdigit()))
            .map(|(a, b)| {
                let hx = format!("{}{}", a, b);
                let byte = u8::from_str_radix(&hx, 16).unwrap();
                byte as char
            }),
    )));

    //  chars from 32-256 (excluding DEL, ', \)
    let raw_char = any().filter(|c: &char| {
        let x = *c as u32;

        (0x20..=0x7E).contains(&x)
            && x != 0x27   // '
            && x != 0x5C   // '\'
        ||
        (0x80..=0xFF).contains(&x)
    });

    let gon = just("\\") // multiline separator
        .ignore_then(gap())
        .ignore_then(just("/"))
        .ignored()
        .labelled("Cord Multiline Separator");

    let char_in_singled_quoted = choice((escape, raw_char)).labelled("Cord Character");

    let single_quoted = char_in_singled_quoted
        .then_ignore(gon.or_not())
        .repeated()
        .collect::<Vec<char>>()
        .delimited_by(just("'"), just("'"))
        .map(cord_chars_to_atom);

    let prefix_spaces = just(' ').repeated();

    let triple_quoted_open = just("'''")
        .map_with(move |_, extra| {
            let span: SimpleSpan = extra.span(); // get identation
            let (_line, col) = linemap.line_col(span.start);
            if col != 0 {
                return (col - 1) as usize;
            }
            return 0 as usize;
        })
        .then_ignore(vul().or(newline()));

    let triple_quoted_close = newline()
        .ignore_then(just(' ').repeated().count())
        .then_ignore(just("'''"))
        .boxed();

    let triple_quoted_content = non_control_char().repeated().collect::<Vec<char>>().boxed();

    let triple_quoted_first_line = triple_quoted_close
        .clone()
        .not()
        .ignore_then(just(' ').repeated().count())
        .then(triple_quoted_content.clone());

    let triple_quoted_line = triple_quoted_close.clone().not().ignore_then(
        newline()
            .ignore_then(just(' ').repeated().count())
            .then(triple_quoted_content),
    );

    let triple_quoted = prefix_spaces
        .ignore_then(triple_quoted_open)
        .then(triple_quoted_first_line.then(triple_quoted_line.repeated().collect::<Vec<_>>()))
        .then(triple_quoted_close)
        .validate(
            |((absolute_indent, (first, mut rest)), close_indent), extra, emit| {
                let span = extra.span();

                if close_indent != absolute_indent {
                    emit.emit(Rich::custom(span, "closing delimiter indentation mismatch"));
                    return Vec::new();
                }
                rest.insert(0, first);

                let mut out: Vec<char> = vec![];
                for (mut indent, mut line) in rest {
                    if indent > absolute_indent {
                        let extra = indent - absolute_indent;
                        indent = absolute_indent;
                        //  extra whitespaces belongs longs to line not indentation
                        line.splice(0..0, std::iter::repeat(' ').take(extra));
                    }

                    //  if line is just a linebreak allow it
                    if indent != absolute_indent && !(line.is_empty() && (indent == 0 as usize)) {
                        emit.emit(Rich::custom(
                            span, "inconsistent indentation in multiline cord",
                        ));
                        return Vec::new();
                    }
                    out.push('\n');
                    if !line.is_empty() {
                        out.extend(line);
                    }
                }
                out.remove(0);
                out
            },
        )
        .map(cord_chars_to_atom);

    choice((triple_quoted, single_quoted)).labelled("Cord")
}

pub fn increment<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('.')
        .or_not()
        .ignore_then(just("+"))
        .ignore_then(just('('))
        .ignore_then(hoon_wide.clone())
        .then_ignore(just(')'))
        .map(|h| Hoon::DotLus(Box::new(h)))
        .labelled("Increment: +(p)")
}

pub fn function_call<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    just('(')
        .ignore_then(hoon.clone())
        .then(
            just(' ')
                .ignore_then(hoon.clone())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .then_ignore(just(')'))
        .map(|(func, args)| Hoon::CenCol(Box::new(func), args))
        .labelled("Function Call")
}

const YEAR_OFFSET: u64 = 292_277_024_400;

fn yelp(yer: u64) -> bool {
    (yer % 4 == 0) && ((yer % 100 != 0) || (yer % 400 == 0))
}

// Constants from ++yo
const CETY: u64 = 36_524; // days in 100 years (non-leap century)
const DAY: u64 = 86_400; // seconds/day
const ERA: u64 = 146_097; // days in 400 years
const HOR: u64 = 3_600; // seconds/hour
const MIT: u64 = 60; // seconds/minute
const MOH: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]; // normal
const MOY: [u64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]; // leap

// ++yawn: days since "Jesus" (proleptic Gregorian)
fn yawn(mut yer: u64, mut mot: u64, mut day: u64) -> u64 {
    // => .(mot (dec mot), day (dec day))
    mot = mot.saturating_sub(1);
    day = day.saturating_sub(1);

    let cah = if yelp(yer) { &MOY } else { &MOH };
    for i in 0..mot as usize {
        day += cah[i];
    }

    loop {
        if yer % 4 != 0 {
            if yer == 0 {
                break;
            }
            yer -= 1;
            day += if yelp(yer) { 366 } else { 365 };
            continue;
        }
        if yer % 100 != 0 {
            if yer < 4 {
                break;
            }
            yer -= 4;
            day += if yelp(yer) { 1_461 } else { 1_460 };
            continue;
        }
        if yer % 400 != 0 {
            if yer < 100 {
                break;
            }
            yer -= 100;
            day += if yelp(yer) { 36_525 } else { 36_524 };
            continue;
        }
        // divisible by 400
        day += (yer / 400) * (1 + 4 * CETY); // 1 + 4*36524 = 146097 = ERA
        break;
    }
    day
}

pub fn apply_sign(a: bool, b: ParsedAtom) -> ParsedAtom {
    match b {
        ParsedAtom::Small(n) => {
            let out = if a {
                2 * n
            } else if n == 0 {
                0
            } else {
                2 * (n - 1) + 1
            };
            ParsedAtom::Small(out)
        }
        ParsedAtom::Big(n) => {
            let out = if a {
                &n << 1
            } else if n.is_zero() {
                num_bigint::BigUint::from(0u32)
            } else {
                ((&n - 1u32) << 1) + 1u32
            };
            ParsedAtom::Big(out)
        }
    }
}

///  Alphanumeric with hyphens
///      Start with a lowercase letter
///      Followed by zero or more: lowercase letter, digit, or hyphen
///
pub fn symbol<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .then(
            any()
                .filter(|c: &char| matches!(c, 'a'..='z' | '0'..='9' | '-'))
                .repeated()
                .collect::<Vec<char>>(),
        )
        .map(|(first, rest)| {
            let mut s = String::with_capacity(rest.len() + 1);
            s.push(first);
            s.extend(rest);
            s
        })
        .labelled("Term")
}

const BTC_BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn build_yek() -> [u8; 256] {
    let mut yek = [0xFFu8; 256];
    for (i, ch) in BTC_BASE58.chars().enumerate() {
        let idx = ch as u8 as usize;
        if idx < 256 {
            yek[idx] = i as u8;
        }
    }
    yek
}

fn cha_fa(yek: &[u8; 256], ch: char) -> Option<u8> {
    let idx = ch as u32;
    if idx > 255 {
        return None;
    }
    let val = yek[idx as usize];
    if val == 0xFF {
        None
    } else {
        Some(val)
    }
}

fn bass_58(digits: &[u8]) -> BigUint {
    digits
        .iter()
        .fold(BigUint::from(0u32), |acc, &d| &acc * 58u32 + d as u32)
}

fn tok(a: &ParsedAtom) -> ParsedAtom {
    let b = pad_fa(&a);

    let swapped = swp(3, a);

    let padded = lsh(3, b, &swapped);

    let len = b + met(3, a);

    let hashed = shay(len as u64, &padded.to_biguint());

    let double_hashed = &ParsedAtom::Big(shay(32, &hashed));
    let truncated = end(3, 4, double_hashed);

    let n = net(5, &truncated);
    n
}

pub fn shay(len: u64, ruz: &BigUint) -> BigUint {
    let len = len as usize;

    let ruz_bytes = ruz.to_bytes_le();
    let msg_len = ruz_bytes.len();

    let mut msg = vec![0u8; len];

    if len == 0 {
    } else if msg_len >= len {
        msg.copy_from_slice(&ruz_bytes[..len]);
    } else {
        msg[..msg_len].copy_from_slice(&ruz_bytes);
    }

    let mut hasher = Sha256::new();
    hasher.update(&msg);
    let digest = hasher.finalize();

    BigUint::from_bytes_le(&digest)
}

fn swp(bloq: usize, b: &ParsedAtom) -> ParsedAtom {
    let blocks = rip(bloq, b);
    let rev = flop(&blocks);
    rep(bloq, None, &rev)
}

fn rip(bloq: usize, b: &ParsedAtom) -> Vec<ParsedAtom> {
    if b.is_zero() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut cur = b.clone();

    while !cur.is_zero() {
        out.push(end(bloq, 1, &cur));
        cur = rsh(bloq, 1, &cur);
    }

    out
}

pub fn den_fa(a: &ParsedAtom) -> Option<ParsedAtom> {
    let b = rsh(3, 4, a);

    if tok(&b) == end(3, 4, a) {
        Some(b)
    } else {
        None
    }
}

fn sit(a: usize, b: &ParsedAtom) -> ParsedAtom {
    end(a, 1, b)
}

//  flip byte endianness
fn net(a: usize, b: &ParsedAtom) -> ParsedAtom {
    let b = sit(a, b);

    if a <= 3 {
        return b;
    }

    let c: usize = a - 1;

    let hi_bit = cut(c, 0, 1, &b);
    let hi = net(c, &hi_bit);

    let lo_bit = cut(c, 1, 1, &b);
    let lo = net(c, &lo_bit);

    let res = con_atoms(lsh(c, 1, &hi), lo);
    res
}

fn met_big(bloq: u32, atom: &BigUint) -> u32 {
    let bits = 1u32 << bloq; // bloq_bits
    if atom.is_zero() {
        return 1;
    }
    let atom_bits = atom.bits() as u32;
    (atom_bits + bits - 1) / bits
}

/// pad(a): number of zero bytes needed to pad `a` to 21 bytes
fn pad_fa_big(a: &BigUint) -> usize {
    let b = met(3, &ParsedAtom::Big(a.clone()));
    if b >= 21 {
        0
    } else {
        21 - b as usize
    }
}

pub fn pad_fa(atom: &ParsedAtom) -> usize {
    21usize.saturating_sub(met(3, atom))
}

pub fn enc_fa(atom: &ParsedAtom) -> ParsedAtom {
    let a = atom;

    let shifted = lsh(3, 4, a).to_biguint();
    let checksum = tok(atom).to_biguint();

    ParsedAtom::from_biguint(shifted ^ checksum)
}

pub fn bitcoin_address<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just("0c")
        .ignore_then(alphanumeric())
        .labelled("Bitcoin Address")
}

pub fn urs<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    any()
        .filter(|c: &char| matches!(c, '0'..='9' | 'a'..='z' | '.' | '_' | '~' | '-'))
        .repeated()
        .collect::<String>()
        .map(string_to_atom)
}

pub fn urt<'src>() -> impl Parser<'src, &'src str, &'src str, Err<'src>> {
    any()
        .filter(|c: &char| matches!(c, '0'..='9' | 'a'..='z' | '.' | '~' | '-'))
        .repeated()
        .at_least(1)
        .to_slice()
}

fn wick(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '~' {
            match chars.next() {
                Some('~') => out.push('~'),    // ~~ -> ~
                Some('-') => out.push('_'),    // ~- -> _
                Some(_) | None => return None, // invalid escape
            }
        } else {
            // Only allow valid @ta characters: [a-z0-9._-]
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-' {
                out.push(c);
            } else {
                return None; // invalid char in atom
            }
        }
    }

    Some(out)
}

pub fn urx<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let hex_escape = any()
        .filter(|c: &char| c.is_ascii_hexdigit())
        .repeated()
        .at_least(1)
        .collect::<String>()
        .delimited_by(just('~'), just('.'))
        .map(|hex_str: String| {
            let big = BigUint::from_str_radix(&hex_str, 16).unwrap_or_default();
            let value_32 = big.iter_u32_digits().next().unwrap_or(0); // low 32 bits

            let tuft_result = tuft(&ParsedAtom::Small(value_32 as u128));

            match tuft_result {
                ParsedAtom::Small(n) => n,
                ParsedAtom::Big(_) => panic!("tuft overflow"),
            }
        });

    let special = choice((
        just("~~").to(b'~' as u128),
        just("~.").to(b'.' as u128),
        just('.').to(b' ' as u128),
    ));

    let ascii = any()
        .filter(|c: &char| c.is_ascii_digit() || c.is_ascii_lowercase() || *c == '-' || *c == '_')
        .map(|c| c as u128);

    let token = choice((hex_escape, special, ascii));

    token
        .repeated()
        .at_least(1)
        .collect::<Vec<u128>>()
        .map(|chars: Vec<u128>| rap(3, &chars))
}

fn atom_shl(a: &ParsedAtom, bits: usize) -> ParsedAtom {
    if bits == 0 {
        return a.clone();
    }
    match a {
        ParsedAtom::Small(n) => {
            if bits >= 128 {
                ParsedAtom::from_biguint(BigUint::from(*n) << bits)
            } else {
                ParsedAtom::Small(n << bits)
            }
        }
        ParsedAtom::Big(b) => ParsedAtom::from_biguint(b << bits),
    }
}

fn atom_shr(atom: &ParsedAtom, bits: usize) -> ParsedAtom {
    if bits == 0 {
        return atom.clone();
    }
    match atom {
        ParsedAtom::Small(n) => {
            if bits >= 128 {
                ParsedAtom::Small(0)
            } else {
                ParsedAtom::Small(n >> bits)
            }
        }
        ParsedAtom::Big(b) => ParsedAtom::from_biguint(b >> bits),
    }
}

fn atom_mask_low_bits(atom: &ParsedAtom, bits: usize) -> ParsedAtom {
    if bits == 0 {
        return ParsedAtom::Small(0);
    }
    match atom {
        ParsedAtom::Small(n) => {
            if bits >= 128 {
                ParsedAtom::Small(*n)
            } else {
                let mask = (1u128 << bits) - 1;
                ParsedAtom::Small(*n & mask)
            }
        }
        ParsedAtom::Big(b) => {
            if bits <= 128 {
                let mask: u128 = (1u128 << bits) - 1;
                let mut limbs = b.iter_u64_digits();
                let lo = limbs.next().unwrap_or(0);
                let hi = limbs.skip(1).next().unwrap_or(0);
                let low_u128 = ((hi as u128) << 64) | (lo as u128);
                ParsedAtom::Small(low_u128 & mask)
            } else {
                let mask = (BigUint::one() << bits) - BigUint::one();
                ParsedAtom::from_biguint(b & &mask)
            }
        }
    }
}

// tuft: ParsedAtom (codepoint) -> ParsedAtom (UTF-8 bytes, @t)
pub fn tuft(atom: &ParsedAtom) -> ParsedAtom {
    // This builds a little-endian byte list, then rap 3 packs it
    let mut bytes: Vec<u8> = Vec::new();
    let mut a = atom.clone();

    loop {
        // ?: =(`@`0 a)
        if a.is_zero() {
            break;
        }

        // b=(end 5 a)
        let b_atom = end(5, 1, &a);
        let b = b_atom.to_u128().unwrap();

        // c=$(a (rsh 5 a))
        a = rsh(5, 1, &a);

        if b <= 0x7f {
            bytes.push(b as u8);
            continue;
        }

        if b <= 0x7ff {
            bytes.push((0b1100_0000 | cut_u(b, 6, 5)) as u8);
            bytes.push((0b1000_0000 | (b & 0x3f)) as u8);
            continue;
        }

        if b <= 0xffff {
            bytes.push((0b1110_0000 | cut_u(b, 12, 4)) as u8);
            bytes.push((0b1000_0000 | cut_u(b, 6, 6)) as u8);
            bytes.push((0b1000_0000 | (b & 0x3f)) as u8);
            continue;
        }

        bytes.push((0b1111_0000 | cut_u(b, 18, 3)) as u8);
        bytes.push((0b1000_0000 | cut_u(b, 12, 6)) as u8);
        bytes.push((0b1000_0000 | cut_u(b, 6, 6)) as u8);
        bytes.push((0b1000_0000 | (b & 0x3f)) as u8);
    }

    // rap 3: pack bytes little-endian into @t
    let mut acc: u128 = 0;
    for (i, byte) in bytes.iter().enumerate() {
        acc |= (*byte as u128) << (i * 8);
    }

    ParsedAtom::Small(acc)
}
// --- Extract low byte as u8 ---
fn atom_to_u8(atom: &ParsedAtom) -> u8 {
    match end(3, 1, atom) {
        ParsedAtom::Small(n) => n as u8,
        ParsedAtom::Big(_) => 0,
    }
}

// --- UTF-8 continuation byte check ---
fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

// --- teff: UTF-8 leading byte → length (1–4) ---
fn teff(atom: &ParsedAtom) -> usize {
    let b = atom_to_u8(atom);
    if b == 0 {
        return 0;
    }
    if b <= 0x7F {
        1
    } else if b <= 0xDF {
        2
    } else if b <= 0xEF {
        3
    } else if b <= 0xF4 {
        4
    } else {
        1
    } // invalid → skip 1 byte
}

// --- Decode one UTF-8 codepoint ---
fn decode_one_utf8(atom: &ParsedAtom, len: usize) -> u32 {
    match len {
        1 => atom_to_u8(atom) as u32,
        2 => {
            let b0 = atom_to_u8(atom);
            let b1 = atom_to_u8(&rsh(3, 1, atom));
            if !is_continuation(b1) {
                return 0xFFFD;
            }
            let cp = ((b0 & 0x1F) as u32) << 6 | (b1 & 0x3F) as u32;
            if cp < 0x80 {
                0xFFFD
            } else {
                cp
            }
        }
        3 => {
            let b0 = atom_to_u8(atom);
            let b1 = atom_to_u8(&rsh(3, 1, atom));
            let b2 = atom_to_u8(&rsh(3, 2, atom));
            if !is_continuation(b1) || !is_continuation(b2) {
                return 0xFFFD;
            }
            let cp = ((b0 & 0x0F) as u32) << 12 | ((b1 & 0x3F) as u32) << 6 | (b2 & 0x3F) as u32;
            if cp < 0x800 || (0xD800..=0xDFFF).contains(&cp) {
                0xFFFD
            } else {
                cp
            }
        }
        4 => {
            let b0 = atom_to_u8(atom);
            let b1 = atom_to_u8(&rsh(3, 1, atom));
            let b2 = atom_to_u8(&rsh(3, 2, atom));
            let b3 = atom_to_u8(&rsh(3, 3, atom));
            if !is_continuation(b1) || !is_continuation(b2) || !is_continuation(b3) {
                return 0xFFFD;
            }
            let cp = ((b0 & 0x07) as u32) << 18
                | ((b1 & 0x3F) as u32) << 12
                | ((b2 & 0x3F) as u32) << 6
                | (b3 & 0x3F) as u32;
            if !(0x1_0000..=0x10_FFFF).contains(&cp) {
                0xFFFD
            } else {
                cp
            }
        }
        _ => 0xFFFD,
    }
}

// @t (UTF-8 atom) -> @c (UTF-32 packed atom)
pub fn taft(atom: &ParsedAtom) -> ParsedAtom {
    let mut codepoints = Vec::new();
    let mut current = atom.clone();

    loop {
        let len = teff(&current);
        if len == 0 {
            break;
        }
        let cp = decode_one_utf8(&current, len);
        codepoints.push(cp);
        current = rsh(3, len, &current); // shift by `len` bytes
    }

    // Pack into @c: each u32 in 32-bit lane, LSB-first (rap 5)
    if codepoints.is_empty() {
        ParsedAtom::Small(0)
    } else if codepoints.len() <= 4 {
        let mut acc: u128 = 0;
        for (i, &cp) in codepoints.iter().enumerate() {
            acc |= (cp as u128) << (i * 32);
        }
        ParsedAtom::Small(acc)
    } else {
        let mut acc = BigUint::zero();
        for (i, &cp) in codepoints.iter().enumerate() {
            acc |= BigUint::from(cp) << (i * 32);
        }
        ParsedAtom::from_biguint(acc)
    }
}

pub fn binary_number<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let bit = any().filter(|c: &char| *c == '0' || *c == '1');

    let first_group = just('0').to("0".to_string()).or(just('1')
        .then(bit.repeated().at_most(3).collect::<String>())
        .map(|(h, t)| h.to_string() + &t));

    let first = just("0b").ignore_then(first_group);

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(bit.repeated().exactly(4).collect::<String>());

    first
        .then(rest.repeated().collect::<Vec<String>>())
        .map(|(first, rest)| {
            if rest.is_empty() {
                first
            } else {
                let mut s = first;
                for r in rest {
                    s.push_str(&r);
                }
                s
            }
        })
        .labelled("Binary")
}

pub fn hexadecimal_number<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let hex = any().filter(|c: &char| c.is_ascii_hexdigit());

    let first_group = hex
        .then(hex.repeated().at_most(3).collect::<String>())
        .map(|(head, tail)| {
            if head == '0' && !tail.is_empty() {
                String::new()
            } else {
                let mut s = String::new();
                s.push(head);
                s.push_str(&tail);
                s
            }
        })
        .filter(|s| !s.is_empty());

    let first = just("0x").ignore_then(first_group);

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(hex.repeated().exactly(4).collect::<String>())
        .repeated()
        .collect::<Vec<String>>();

    first
        .then(rest)
        .map(|(first, rest)| {
            if rest.is_empty() {
                first
            } else {
                let mut s = first;
                for r in rest {
                    s.push_str(&r);
                }
                s
            }
        })
        .labelled("Hexadecimal")
}

pub fn ipv4_address<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let octet = any()
        .filter(|c: &char| c.is_ascii_digit())
        .repeated()
        .at_least(1)
        .at_most(3)
        .collect::<String>()
        .filter(|s: &String| {
            if s.is_empty() || s.starts_with('0') && s.len() > 1 {
                return false;
            }
            let n = s.parse::<u16>().unwrap_or(256);
            n <= 255
        });

    octet
        .separated_by(just('.').ignore_then(gap().or_not()))
        .exactly(4)
        .collect::<Vec<String>>()
        .map(|parts| parts.join("."))
        .labelled("IPv4-Address")
}

pub fn ipv6_address<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(alphanumeric())
        .repeated()
        .exactly(7)
        .collect::<Vec<_>>();

    alphanumeric()
        .then(rest)
        .map(|(first, mut rest)| {
            if rest.is_empty() {
                first.to_string()
            } else {
                let mut parts = vec![first];
                parts.append(&mut rest);
                parts.join(":").to_string()
            }
        })
        .labelled("Ipv6-Address")
}

pub fn base32_number<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let base32_digit = any().filter(|c: &char| c.is_ascii_digit() || ('a'..='v').contains(c));

    let first = just("0v").ignore_then(choice((
        just('0').to("0".to_string()),
        any()
            .filter(|c: &char| matches!(c, '1'..='9' | 'a'..='v'))
            .then(base32_digit.repeated().at_most(4).collect::<String>())
            .map(|(h, t)| h.to_string() + &t),
    )));

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(base32_digit.repeated().exactly(5).collect::<String>())
        .repeated()
        .collect::<Vec<String>>();

    first
        .then(rest)
        .map(|(first, mut rest)| {
            if rest.is_empty() {
                base32_to_atom(first.to_string())
            } else {
                let mut parts = vec![first];
                parts.append(&mut rest);
                base32_to_atom(parts.join(""))
            }
        })
        .labelled("Base32")
}

pub fn base64_number<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let digit = any().filter(|c: &char| matches!(c, '0'..='9' | 'a'..='z' | 'A'..='Z' | '-' | '~'));

    let first = just("0w").ignore_then(
        just('0').to("0".to_string()).or(any()
            .filter(|c: &char| matches!(c, '1'..='9' | 'a'..='z' | 'A'..='Z' | '-' | '~'))
            .then(digit.repeated().at_most(4).collect::<String>())
            .map(|(h, t)| h.to_string() + &t)),
    );

    let group = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(digit.repeated().exactly(5).collect::<String>());

    first
        .then(group.repeated().collect::<Vec<String>>())
        .map(|(first, rest)| {
            if rest.is_empty() {
                base64_to_atom(first)
            } else {
                let mut parts = vec![first];
                parts.extend(rest);
                base64_to_atom(parts.join(""))
            }
        })
        .labelled("Base64")
}

pub fn base32<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_alphanumeric() && *c <= 'v')
        .repeated()
        .at_least(1)
        .collect::<String>()
}

pub fn digits<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_digit())
        .repeated()
        .at_least(1)
        .collect::<String>()
}

pub fn alphanumeric<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_alphanumeric())
        .repeated()
        .at_least(1)
        .collect::<String>()
}

pub fn decimal_number<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    let digit = any().filter(|c: &char| c.is_ascii_digit());

    let non_zero_digit = any().filter(|c: &char| matches!(c, '1'..='9'));

    let first = just('0').to("0".to_string()).or(non_zero_digit
        .then(digit.repeated().at_most(2).collect::<Vec<char>>())
        .map(|(h, t)| {
            let mut s = String::with_capacity(3);
            s.push(h);
            s.extend(t);
            s
        }));

    let three_digits = digit.repeated().exactly(3).collect::<String>();

    let rest = just('.')
        .ignore_then(gap().or_not())
        .ignore_then(three_digits)
        .repeated()
        .collect::<Vec<String>>();

    first
        .then(rest)
        .map(|(first_digits, rest_digits)| {
            let mut out = first_digits;
            for chunk in rest_digits {
                out.push_str(&chunk);
            }
            out
        })
        .labelled("Decimal Number")
}

fn snag<T>(index: usize, list: &[T]) -> &T {
    list.get(index).expect("snag: index out of bounds")
}

pub fn weld<T: Clone>(a: impl AsRef<[T]>, b: impl AsRef<[T]>) -> Vec<T> {
    let a = a.as_ref();
    let b = b.as_ref();
    let mut v = Vec::with_capacity(a.len() + b.len());
    v.extend_from_slice(a);
    v.extend_from_slice(b);
    v
}

pub fn scag<T: Clone>(n: usize, list: impl AsRef<[T]>) -> Vec<T> {
    list.as_ref().iter().take(n).cloned().collect()
}

pub fn slag<T: Clone>(n: usize, list: impl AsRef<[T]>) -> Vec<T> {
    list.as_ref().iter().skip(n).cloned().collect()
}

pub fn flop<T: Clone>(list: impl AsRef<[T]>) -> Vec<T> {
    let mut v = list.as_ref().to_vec();
    v.reverse();
    v
}

fn poof(pax: Path) -> Vec<Hoon> {
    pax.iter()
        .map(|a| {
            Hoon::Sand(
                "ta".to_string(),
                NounExpr::ParsedAtom(string_to_atom(a.clone())),
            )
        })
        .collect()
}

// used to create dbug traces
#[derive(Clone)]
pub struct LineMap {
    starts: Vec<usize>,
}

impl LineMap {
    #[inline]
    pub fn new(src: &str) -> Self {
        let mut starts = Vec::with_capacity(128);
        starts.push(0);

        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }

        Self { starts }
    }

    #[inline(always)]
    fn line_col(&self, byte: usize) -> (u64, u64) {
        let line = match self.starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i - 1,
        };

        ((line + 1) as u64, (byte - self.starts[line] + 1) as u64)
    }

    #[inline(always)]
    pub fn pint(&self, span: std::ops::Range<usize>) -> Pint {
        Pint {
            p: self.line_col(span.start),
            q: self.line_col(span.end),
        }
    }
}

fn poon(pag: &[Hoon], goo: &[Option<Hoon>]) -> Option<Vec<Hoon>> {
    if goo.is_empty() {
        return Some(vec![]);
    }

    let (goo_hd, goo_tl) = goo.split_first().unwrap();

    let head = match goo_hd {
        Some(x) => x.clone(),
        None => {
            let (pag_hd, _) = pag.split_first()?;
            pag_hd.clone()
        }
    };

    let pag_tl = if pag.is_empty() { &[] } else { &pag[1..] };

    let mut rest = poon(pag_tl, goo_tl)?;

    let mut out = Vec::with_capacity(rest.len() + 1);
    out.push(head);
    out.append(&mut rest);

    Some(out)
}

pub fn posh(
    pre: Option<Vec<Option<Hoon>>>,          // (unit tyke)
    pof: Option<(usize, Vec<Option<Hoon>>)>, // (unit [p=@ud q=tyke])
    wer: Path,
) -> Option<Vec<Hoon>> {
    let wom: Vec<Hoon> = poof(wer);

    let yez = if pre.is_none() {
        Some(wom.clone())
    } else {
        let pre_val = pre.as_ref().unwrap();

        let moz = poon(&wom, pre_val)?;

        if let Some(_) = pof {
            let n = pre_val.len();
            let sl = slag(n, &wom.clone());
            Some(weld(&moz, &sl))
        } else {
            Some(moz)
        }
    }?;

    if pof.is_none() {
        return Some(yez);
    }

    let (p, q) = pof.unwrap();

    let zey = flop(&yez.clone());

    let moz = scag(p, &zey);
    let gul = slag(p, &zey);

    let zom = poon(&flop(&moz.clone()), &q);

    match zom {
        None => None,
        Some(z) => Some(weld(&flop(&gul), z)),
    }
}

pub fn nusk<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    urt()
        .try_map(|s, span| {
            wick(s).ok_or_else(|| Rich::custom(span, format!("invalid knot escape in '{}'", s)))
        })
        .try_map(|unescaped: String, span| {
            let parsed = nuck().parse(&unescaped);
            match parsed.into_result() {
                Ok(output) => Ok(output),
                Err(_errors) => Err(Rich::custom(span, "nuck parse failed")),
            }
        })
}

pub fn jock(rad: bool, lot: &Coin) -> Hoon {
    match lot {
        Coin::Dime(tag, atom) => {
            if rad {
                Hoon::Rock(tag.clone(), NounExpr::ParsedAtom(atom.clone()))
            } else {
                Hoon::Sand(tag.clone(), NounExpr::ParsedAtom(atom.clone()))
            }
        }

        Coin::Blob(noun) => {
            if rad {
                Hoon::Rock("$".to_string(), noun.clone())
            } else {
                match noun {
                    NounExpr::ParsedAtom(atom) => {
                        Hoon::Sand("$".to_string(), NounExpr::ParsedAtom(atom.clone()))
                    }
                    NounExpr::Cell(head, tail) => Hoon::Pair(
                        Box::new(jock(rad, &Coin::Blob(*head.clone()))),
                        Box::new(jock(rad, &Coin::Blob(*tail.clone()))),
                    ),
                }
            }
        }

        Coin::Many(coins) => Hoon::ColTar(coins.iter().map(|c| jock(rad, c)).collect()),
    }
}

pub fn nuck<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        symbol().map(|s| Coin::Dime("tas".to_string(), string_to_atom(s))),
        number().map(|(p, q)| Coin::Dime(p, q)),
        just('.').ignore_then(perd()),
        just('~').ignore_then(choice((
            twid(),
            empty().to(Coin::Dime("n".to_string(), ParsedAtom::Small(0))),
        ))),
    ))
    .boxed()
}

pub fn perd<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        zust(),
        nusk()
            .separated_by(just('_'))
            .at_least(1)
            .collect::<Vec<_>>()
            .delimited_by(just('_'), just("__"))
            .map(|t| Coin::Many(t)),
    ))
}

pub fn zust<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        ipv6_address().try_map(|s, span| {
            let maybe_ipv6 = ipv6_to_atom(s.clone());
            match maybe_ipv6 {
                None => Err(Rich::custom(span, "invalid ipv6")),
                Some(atom) => Ok(Coin::Dime("is".to_string(), atom)),
            }
        }),
        ipv4_address().try_map(|s, span| {
            let maybe_ipv4 = ipv4_to_atom(s);
            match maybe_ipv4 {
                None => Err(Rich::custom(span, "invalid ipv4")),
                Some(atom) => Ok(Coin::Dime("if".to_string(), atom)),
            }
        }),
        float().map(|(p, q)| Coin::Dime(p, q)),
        just("y").to(Coin::Dime("f".to_string(), ParsedAtom::Small(0))),
        just("n").to(Coin::Dime("f".to_string(), ParsedAtom::Small(1))),
        just('~')
            .ignore_then(phonemic_name_unscrambled())
            .map(|s| Coin::Dime("q".to_string(), s)),
    ))
}

pub fn trip(mut atom: ParsedAtom) -> Tape {
    let mut out = Vec::new();

    while atom != ParsedAtom::Small(0) {
        let byte_atom = end(3, 1, &atom);

        let byte = match byte_atom {
            ParsedAtom::Small(x) => x as u8,
            ParsedAtom::Big(b) => b.try_into().unwrap_or(0),
        };

        out.push((byte as char).to_string());
        atom = rsh(3, 1, &atom);
    }

    out
}

pub fn wack(a: &str) -> String {
    a.chars()
        .flat_map(|c| match c {
            '~' => vec!['~', '~'],
            '_' => vec!['~', '-'],
            _ => vec![c],
        })
        .collect()
}

pub fn reap<T: Clone>(a: usize, b: T) -> Vec<T> {
    vec![b; a]
}

pub fn path<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    let wer1 = wer.clone();
    let wer2 = wer.clone();
    let wer3 = wer.clone();
    let wer4 = wer.clone();

    let hasp = choice((
        hoon_wide.clone().delimited_by(just('['), just(']')),
        hoon_wide
            .clone()
            .separated_by(just(' '))
            .at_least(1)
            .collect::<Vec<_>>()
            .delimited_by(just('('), just(')'))
            .map(|list| {
                let (first, rest) = list.split_first().unwrap();
                Hoon::CenCol(Box::new(first.clone()), rest.to_vec())
            }),
        just('$').to(Hoon::Sand(
            "tas".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(0)),
        )),
        cord(linemap).map(|s| Hoon::Sand("t".to_string(), NounExpr::ParsedAtom(s))),
        nuck().map(|coin| {
            let aura = match &coin {
                Coin::Dime(a, _) if a == "tas" => "tas",
                _ => "ta",
            };
            Hoon::Sand(aura.to_string(), NounExpr::ParsedAtom(rent_co(&coin)))
        }),
    ));

    let gasp = choice((
        just('=')
            .to(None)
            .repeated()
            .collect::<Vec<Option<Hoon>>>()
            .then(hasp.map(|h| vec![Some(h)]))
            .then(just('=').to(None).repeated().collect::<Vec<Option<Hoon>>>())
            .map(|((mut a, b), c)| {
                a.extend(b);
                a.extend(c);
                a
            }),
        just('=')
            .to(None)
            .repeated()
            .at_least(1)
            .collect::<Vec<Option<Hoon>>>(),
    ));

    let limp = just("/").repeated().count().then(gasp).map(|(a, mut b)| {
        for _ in 0..a {
            b.insert(
                0,
                Some(Hoon::Sand(
                    "tas".to_string(),
                    NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                )),
            );
        }
        b
    });

    let gash = limp
        .separated_by(just("/"))
        .collect::<Vec<Vec<Option<Hoon>>>>()
        .map(|a| a.into_iter().flatten().collect::<Vec<_>>())
        .boxed();

    let porc = just("%")
        .repeated()
        .count() //  usize
        .then(just("/").ignore_then(gash.clone())); // Vec<Option<Hoon>>

    let poor = gash
        .clone()
        .map(|pre| Some(pre))
        .then(just("%").ignore_then(porc.clone()).or_not());

    let rood = {
        just("/")
            .ignore_then(
                poor.try_map(move |(pre, pof), span| match posh(pre, pof, wer1.clone()) {
                    Some(list) => Ok(Hoon::ColSig(list)),
                    None => Err(Rich::custom(span, "error parsing path")),
                }),
            )
            .labelled("Path")
    };

    let cen_fas = {
        porc.try_map(
            move |(a, b), span| match posh(Some(vec![None]), Some((a, b)), wer2.clone()) {
                Some(list) => Ok(Hoon::ColSig(list)),
                None => Err(Rich::custom(span, "error parsing path")),
            },
        )
    };

    let multi_cen = {
        just("%").repeated().count().try_map(move |n, span| {
            match posh(Some(vec![None]), Some((n, vec![])), wer3.clone()) {
                Some(list) => Ok(Hoon::ColSig(list)),
                None => Err(Rich::custom(span, "error parsing path")),
            }
        })
    };

    let cen_path = just("%")
        .ignore_then(choice((cen_fas, multi_cen)))
        .labelled("Path");

    choice((
        rood.boxed(),     //  /foo/%/foo
        cen_path.boxed(), //  %/foo  and  %%
    ))
    .labelled("Path")
}

pub fn rent_co(lot: &Coin) -> ParsedAtom {
    let rend_res = rend_co(lot);
    let bytes: Vec<u128> = rend_res
        .into_iter()
        .flat_map(|s: String| s.chars().map(|c| c as u128).collect::<Vec<_>>())
        .collect();
    let rap_res = rap(3 as usize, &bytes);
    rap_res
}

pub fn rend_co(lot: &Coin) -> Tape {
    rend_with_rep(lot, vec![])
}

fn rend_many(coins: &[Coin], rep: Tape) -> Tape {
    if coins.is_empty() {
        return vec!["_".to_string(), "_".to_string()]
            .into_iter()
            .chain(rep)
            .collect();
    }
    let first = &coins[0];
    let rest = &coins[1..];

    let mut res = vec!["_".to_string()];
    let rendered_first = rend_co(first);
    let escaped_knot = wack(&rendered_first.concat());
    let taped_escaped = trip(string_to_atom(escaped_knot));
    res.extend(taped_escaped);
    res.extend(rend_many(rest, rep));
    res
}

fn rend_with_rep(lot: &Coin, mut rep: Tape) -> Tape {
    match lot {
        Coin::Blob(noun) => {
            let jammed = jam_simple(noun.clone());
            let mut res = vec!["~".to_string(), "0".to_string()];
            res.extend(v_co(1, &jammed));
            res
        }

        Coin::Many(coins) => {
            let mut res = vec![".".to_string()];
            res.extend(rend_many(coins, rep));
            res
        }

        Coin::Dime(prefix, q) => {
            let yed = end(3, 1, &string_to_atom(prefix.to_string())); // first char of prefix
            let hay = cut(3, 1, 1, &string_to_atom(prefix.to_string())); // second char

            let yed_char = match &yed {
                ParsedAtom::Small(x) => *x as u8 as char,
                ParsedAtom::Big(_) => unreachable!(), // prefix is short
            };

            let hay_char = match &hay {
                ParsedAtom::Small(x) => *x as u8 as char,
                ParsedAtom::Big(_) => unreachable!(),
            };

            match yed_char {
                'c' => {
                    let mut res = vec!['~'.to_string(), '-'.to_string()];
                    let wood_res = wood(&tuft(q));
                    let rip_res = rip(3, &wood_res);
                    let qtape: Vec<_> = rip_res.into_iter().flat_map(|a| trip(a)).collect();
                    res.extend(qtape);
                    res.extend(rep);
                    res
                }

                'd' => match hay_char {
                    'a' => {
                        let yod = yore(q);
                        let mut rep = rep;
                        if !yod.t.f.is_empty() {
                            let frac_tape = s_co(&yod.t.f);
                            let mut new_rep = vec![".".to_string()];
                            new_rep.extend(frac_tape);
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        let t = &yod.t;
                        if !(yod.t.f.is_empty() && t.h == 0 && t.m == 0 && t.s == 0) {
                            let s_atom = ParsedAtom::Small(t.s as u128);
                            let mut new_rep = vec![".".to_string()];
                            new_rep.extend(y_co(&s_atom));
                            let m_atom = ParsedAtom::Small(t.m as u128);
                            let mut newer_rep = vec![".".to_string()];
                            newer_rep.extend(y_co(&m_atom));
                            newer_rep.extend(new_rep);
                            let h_atom = ParsedAtom::Small(t.h as u128);
                            let mut newest_rep = vec![".".to_string(), ".".to_string()];
                            newest_rep.extend(y_co(&h_atom));
                            newest_rep.extend(newer_rep);
                            newest_rep.extend(rep);
                            rep = newest_rep
                        }

                        let d_atom = ParsedAtom::Small(t.d as u128);
                        let mut new_rep = vec![".".to_string()];
                        new_rep.extend(a_co(&d_atom));
                        new_rep.extend(rep);
                        rep = new_rep;

                        let m_atom = ParsedAtom::Small(yod.m as u128);
                        let mut newer_rep = vec![".".to_string()];
                        newer_rep.extend(a_co(&m_atom));
                        newer_rep.extend(rep);
                        rep = newer_rep;

                        if !yod.era {
                            let mut newest_rep = vec!["-".to_string()];
                            newest_rep.extend(rep);
                            rep = newest_rep;
                        }

                        let y_atom = ParsedAtom::Small(yod.y as u128);
                        let mut res = vec!["~".to_string()];
                        res.extend(a_co(&y_atom));
                        res.extend(rep);
                        res
                    }

                    'r' => {
                        let yug = yell(q);

                        let mut rep = rep;

                        if !yug.f.is_empty() {
                            let frac_tape = s_co(&yug.f);
                            let mut new_rep = vec![".".to_string()];
                            new_rep.extend(frac_tape);
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        let mut res = vec!["~".to_string()];

                        if yug.d == 0 && yug.m == 0 && yug.h == 0 && yug.s == 0 {
                            res.extend(vec!["s".to_string(), "0".to_string()]);
                            res.extend(rep);
                            return res;
                        }

                        if yug.s != 0 {
                            let s_atom = ParsedAtom::Small(yug.s as u128);
                            let mut new_rep = vec![".".to_string(), "s".to_string()];
                            new_rep.extend(a_co(&s_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        if yug.m != 0 {
                            let m_atom = ParsedAtom::Small(yug.m as u128);
                            let mut new_rep = vec![".".to_string(), "m".to_string()];
                            new_rep.extend(a_co(&m_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        if yug.h != 0 {
                            let h_atom = ParsedAtom::Small(yug.h as u128);
                            let mut new_rep = vec![".".to_string(), "h".to_string()];
                            new_rep.extend(a_co(&h_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        if yug.d != 0 {
                            let d_atom = ParsedAtom::Small(yug.d as u128);
                            let mut new_rep = vec![".".to_string(), "d".to_string()];
                            new_rep.extend(a_co(&d_atom));
                            new_rep.extend(rep);
                            rep = new_rep;
                        }

                        res.extend(rep.iter().skip(1).cloned());
                        res
                    }

                    _ => z_co(q),
                },

                'f' => match q {
                    ParsedAtom::Small(0) => vec!['.'.to_string(), 'y'.to_string()],
                    ParsedAtom::Small(1) => vec!['.'.to_string(), 'n'.to_string()],
                    _ => z_co(q),
                }
                .into_iter()
                .chain(rep.into_iter())
                .collect(),

                'n' => {
                    let mut res = vec!['~'.to_string()];
                    res.extend(rep);
                    res
                }

                'i' => match hay_char {
                    'f' => ro_co([3, 10, 4], &|x| d_ne(x), q),
                    's' => ro_co([4, 16, 8], &|x| x_ne(x), q),
                    _ => z_co(q),
                },

                'p' => {
                    let sxz = fein(q.clone());
                    let dyx = met(3, &sxz);

                    let mut out: Tape = vec!['~'.to_string()];

                    if dyx <= 1 {
                        let byte = sxz.to_u8_lossy();
                        let syl = tod_po(byte);
                        out.extend(trip(syl));
                        out.extend(rep);
                        return out;
                    }

                    let dyy = met(4, &sxz);
                    let mut chunks = Vec::with_capacity(dyy);

                    for imp in 0..dyy {
                        let log = cut(4, imp, 1, &sxz);

                        let hi_atom = rsh(3, 1, &log);
                        let hi = hi_atom.to_u8_lossy();

                        let lo_atom = end(3, 1, &log);
                        let lo = lo_atom.to_u8_lossy();

                        let prefix = trip(tos_po(hi));
                        let suffix = trip(tod_po(lo));

                        let mut chunk = weld(&prefix, &suffix);

                        let sep = if imp % 4 == 0 {
                            if imp == 0 {
                                vec![]
                            } else {
                                vec!['-'.to_string(), '-'.to_string()]
                            }
                        } else {
                            vec!['-'.to_string()]
                        };
                        chunk.extend(sep);

                        chunks.push(chunk);
                    }

                    chunks.reverse();
                    for chunk in chunks {
                        out.extend(chunk);
                    }
                    out.extend(rep);
                    out
                }

                'q' => {
                    let head = vec![".".to_string(), "~".to_string()];

                    let lot: Vec<ParsedAtom> = if q.is_zero() {
                        vec![ParsedAtom::Small(0)]
                    } else {
                        rip(3, q)
                    };

                    let mut r: Tape = Vec::new();
                    let mut s = true;

                    for atom in lot.into_iter() {
                        let q_atom = atom.to_u8().expect("byte");

                        let mut rendered = if s {
                            trip(tod_po(q_atom))
                        } else {
                            trip(tos_po(q_atom))
                        };

                        let tail = if s && !r.is_empty() {
                            let mut t = vec!["-".to_string()];
                            t.extend(r);
                            t
                        } else {
                            r
                        };

                        s = !s;
                        r = weld(rendered, tail);
                    }

                    let mut res = head;
                    res = weld(res, r);
                    res = weld(res, rep);
                    res
                }

                'r' => match hay_char {
                    'd' => {
                        let val = q.to_u128().unwrap();
                        let df = rlyd(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![".".to_string(), "~".to_string()];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    'h' => {
                        let val = q.to_u128().unwrap();
                        let df = rlyh(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![".".to_string(), "~".to_string(), "~".to_string()];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    'q' => {
                        let val = q.to_u128().unwrap();
                        let df = rlyq(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![
                            ".".to_string(),
                            "~".to_string(),
                            "~".to_string(),
                            "~".to_string(),
                        ];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    's' => {
                        let val = q.to_u128().unwrap();
                        let df = rlys(val);
                        let rc = r_co(&df, rep.clone());
                        let mut res = vec![".".to_string()];
                        res.extend(rc);
                        res.extend(rep);
                        res
                    }
                    _ => {
                        let mut res = z_co(q);
                        res.extend(rep);
                        res
                    }
                },

                'u' => {
                    match hay_char {
                        'c' => {
                            // base58check with padding
                            let encoded = enc_fa(q);
                            let padded_ones = reap(pad_fa(&q), '1'.to_string());
                            let mut res = vec!['0'.to_string(), 'c'.to_string()];
                            res.extend(padded_ones);
                            res.extend(c_co(&encoded));
                            res.extend(rep);
                            res
                        }
                        'b' => with_prefix("0b", &ox_co([2, 4], &|x| d_ne(x), q), rep),
                        'i' => with_prefix("0i", &d_co(1, q), rep),
                        'x' => with_prefix("0x", &ox_co([16, 4], &|x| x_ne(x), q), rep),
                        'v' => with_prefix("0v", &ox_co([32, 5], &|x| x_ne(x), q), rep),
                        'w' => with_prefix("0w", &ox_co([64, 5], &|x| w_ne(x), q), rep),
                        _ => {
                            vec![ox_co([10, 3], &|x| d_ne(x), q)
                                .into_iter()
                                .chain(rep)
                                .collect()]
                        }
                    }
                }

                's' => {
                    let q = q.to_u128().expect("signed number is bigger than 128 bits");
                    let sign_prefix_chars = if syn_si(q) {
                        vec!['-'.to_string(), '-'.to_string()]
                    } else {
                        vec!['-'.to_string()]
                    };
                    let abs_val = abs_si(q);
                    let mut res: Tape = sign_prefix_chars.into_iter().collect();
                    res.extend(rend_with_rep(
                        &Coin::Dime("u".into(), ParsedAtom::Small(abs_val)),
                        rep,
                    ));
                    res
                }

                't' => {
                    if hay_char == 'a' {
                        let third = cut(3, 2, 1, &string_to_atom(prefix.to_string()));
                        let third_char = match &third {
                            ParsedAtom::Small(x) => *x as u8 as char,
                            ParsedAtom::Big(_) => '\0',
                        };
                        if third_char == 's' {
                            let mut res: Vec<_> =
                                rip(3, q).into_iter().flat_map(|a| trip(a)).collect();
                            res.extend(rep);
                            res
                        } else {
                            let mut res = vec!['~'.to_string(), '.'.to_string()];
                            res.extend(rip(3, q).into_iter().flat_map(|a| trip(a)));
                            res.extend(rep);
                            res
                        }
                    } else {
                        let mut res = vec!['~'.to_string(), '~'.to_string()];
                        let wooded = wood(q);
                        res.extend(
                            rip(3, &ParsedAtom::from(wooded))
                                .into_iter()
                                .flat_map(|a| trip(a)),
                        );
                        res.extend(rep);
                        res
                    }
                }

                _ => z_co(q),
            }
        }
    }
}

fn r_co(df: &DecimalFloat, mut rep: Tape) -> Tape {
    match df {
        DecimalFloat::Infinity { sign } => {
            let prefix = if *sign { "inf" } else { "-inf" };
            prefix
                .chars()
                .map(|c| c.to_string())
                .chain(rep.into_iter())
                .collect()
        }
        DecimalFloat::NaN => "nan"
            .chars()
            .map(|c| c.to_string())
            .chain(rep.into_iter())
            .collect(),
        DecimalFloat::Finite { sign, exp, mant } => {
            let f: Tape = d_co(1, &ParsedAtom::Big(mant.clone()));

            let (e, exp): (u128, u128) = {
                let e = sun_si(f.len() as u128);

                let sci = sum_si(*exp, sum_si(e, 1));

                if syn_si(dif_si(*exp, 6)) {
                    (2, sci)
                } else if !syn_si(dif_si(sci, 3)) {
                    (2, sci)
                } else {
                    (sum_si(sci, 2), 0)
                }
            };

            if exp != 0u128 {
                let exp_mark = if syn_si(exp) { "e" } else { "e-" };
                rep = weld(
                    vec![exp_mark.to_string()],
                    d_co(1, &ParsedAtom::Small(abs_si(exp))),
                );
            }

            let mut out = weld(ed_co(&e, &f), rep);

            if !sign {
                out = weld(vec!["-".to_string()], out);
            }

            out
        }
    }
}

fn ed_co(exp: &u128, int: &Tape) -> Tape {
    let cmp = cmp_si(*exp, 0);
    let pos = cmp == 2;
    let dig = abs_si(*exp) as usize;

    if !pos {
        let mut out = reap(dig + 1, "0".to_string());
        out.extend(int.clone());
        return into(out, 1, ".");
    }

    let len = int.len();

    if dig < len {
        return into(int.clone(), dig, ".");
    }

    let mut out = int.clone();
    out.extend(reap(dig - len, "0".to_string()));
    out
}

fn wood_go(a: &ParsedAtom) -> Vec<u128> {
    if a.is_zero() {
        return Vec::new();
    }

    let b = teff(a);
    let c_atom = taft(&end(3, b, a));
    let c = c_atom.to_u32().unwrap();
    let mut d = wood_go(&rsh(3, b, a));

    // alnum or '-'
    if (c >= b'a' as u32 && c <= b'z' as u32)
        || (c >= b'0' as u32 && c <= b'9' as u32)
        || c == b'-' as u32
    {
        d.insert(0, c as u128);
        return d;
    }

    match c as u8 {
        b' ' => {
            d.insert(0, b'.' as u128);
        }
        b'.' => {
            d.insert(0, b'.' as u128);
            d.insert(0, b'~' as u128);
        }
        b'~' => {
            d.insert(0, b'~' as u128);
            d.insert(0, b'~' as u128);
        }
        _ => {
            d = wood_hex(c, d);
        }
    }

    d
}

fn wood_hex(c: u32, mut d: Vec<u128>) -> Vec<u128> {
    let e = met(2, &ParsedAtom::Small(c as u128));

    d.insert(0, b'.' as u128);

    for i in 0..e {
        let shift = i * 4;
        let f = (c >> shift) & 0xF;
        let ch = if f <= 9 { 48 + f } else { 87 + f };
        d.insert(0, ch as u128);
    }

    d.insert(0, b'~' as u128);
    d
}

pub fn wood(a: &ParsedAtom) -> ParsedAtom {
    let bytes = wood_go(a);
    rap(3, &bytes)
}

fn into(mut tape: Tape, idx: usize, ch: &str) -> Tape {
    tape.insert(idx, ch.to_string());
    tape
}

fn atom_to_char(atom: &ParsedAtom) -> char {
    let code = match atom {
        ParsedAtom::Small(x) => *x as u32,
        ParsedAtom::Big(b) => {
            if *b > BigUint::from(u32::MAX) {
                0xFFFD //  replacement
            } else {
                b.clone().try_into().unwrap_or(0xFFFD)
            }
        }
    };
    std::char::from_u32(code).unwrap_or('\u{FFFD}')
}

fn d_ne(tig: u128) -> char {
    (tig as u8 + b'0') as char
}

fn x_ne(tig: u128) -> char {
    if tig < 10 {
        (b'0' + tig as u8) as char
    } else {
        (b'a' + (tig - 10) as u8) as char
    }
}

fn v_ne(tig: u128) -> char {
    if tig >= 10 {
        (tig + 87) as u8 as char
    } else {
        (tig + 48) as u8 as char
    }
}

fn w_ne(tig: u128) -> char {
    // base64 with - and ~ for 62/63
    if tig == 62 {
        '-'
    } else if tig == 63 {
        '~'
    } else if tig < 26 {
        (b'A' + tig as u8) as char
    } else if tig < 52 {
        (b'a' + (tig - 26) as u8) as char
    } else if tig < 62 {
        (b'0' + (tig - 52) as u8) as char
    } else {
        unreachable!()
    }
}

fn c_ne(tig: u128) -> char {
    // base58: skips 0, O, I, l
    const CHARS: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    CHARS[tig as usize] as char
}

fn with_prefix(prefix: &str, body: &Tape, rep: Tape) -> Tape {
    let mut res: Tape = prefix.chars().map(|c| c.to_string()).collect();
    res.extend(body.iter().cloned());
    res.extend(rep);
    res
}

fn s_co(frac: &[u64]) -> Tape {
    if frac.is_empty() {
        return vec![];
    }
    let mut res = vec![".".to_string()];
    let first = ParsedAtom::Small(frac[0] as u128);
    res.extend(x_co(4, &first));
    res.extend(s_co(&frac[1..]));
    res
}

fn em_co<F>(bas: u128, min: usize, mut par: F, hol: &ParsedAtom, rep: Tape) -> Tape
where
    F: FnMut(bool, u128, Tape) -> Tape,
{
    if hol.is_zero() && min == 0 {
        return rep;
    }
    let (dar, rad) = dvr(hol, &ParsedAtom::Small(bas));
    let next_min = min.saturating_sub(1);
    let rad_u128 = rad.to_u128().unwrap_or(0);
    let next_rep = par(dar.is_zero(), rad_u128, rep);
    em_co(bas, next_min, par, &dar, next_rep)
}

// Helper: dvr for ParsedAtom
fn dvr(a: &ParsedAtom, b: &ParsedAtom) -> (ParsedAtom, ParsedAtom) {
    match (a, b) {
        (ParsedAtom::Small(x), ParsedAtom::Small(y)) => {
            let (q, r) = (x / y, x % y);
            (ParsedAtom::Small(q), ParsedAtom::Small(r))
        }
        _ => {
            let a_big = a.to_biguint();
            let b_big = b.to_biguint();
            let (q, r) = dvr_big(&a_big, &b_big);
            (ParsedAtom::Big(q), ParsedAtom::Big(r))
        }
    }
}

fn dvr_u64(a: u64, b: u64) -> (u64, u64) {
    (a / b, a % b)
}

fn d_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        10,
        min,
        |_, b, c: Tape| {
            let ch = d_ne(b);
            std::iter::once(ch.to_string()).chain(c).collect()
        },
        dat,
        vec![],
    )
}

fn x_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        16,
        min,
        |_, b, c| {
            let ch = x_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn v_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        32,
        min,
        |_, b, c| {
            let ch = v_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn w_co(min: usize, dat: &ParsedAtom) -> Tape {
    em_co(
        64,
        min,
        |_, b, c| {
            let ch = w_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn c_co(dat: &ParsedAtom) -> Tape {
    em_co(
        58,
        1,
        |_, b, c| {
            let ch = c_ne(b).to_string();
            std::iter::once(ch).chain(c).collect::<Vec<String>>()
        },
        dat,
        vec![],
    )
}

fn a_co(dat: &ParsedAtom) -> Tape {
    d_co(1, dat)
}

fn y_co(dat: &ParsedAtom) -> Tape {
    d_co(2, dat)
}

fn z_co(dat: &ParsedAtom) -> Tape {
    let mut res = vec!["0".to_string(), "x".to_string()];
    res.extend(x_co(1, dat));
    res
}

fn ox_co<F>([bas, gop]: [u128; 2], dug: &F, hol: &ParsedAtom) -> Tape
where
    F: Fn(u128) -> char,
{
    let pow_bas_gop = pow(bas, gop).to_u128().expect("base does not fit in u128");
    em_co(
        pow_bas_gop,
        0,
        |top, seg, res| {
            let prefix: Tape = if top { vec![] } else { vec!['.'.to_string()] };
            let inner = em_co(
                bas,
                if top { 0 } else { gop as usize },
                |_, b, c| {
                    std::iter::once(dug(b).to_string())
                        .chain(c)
                        .collect::<Vec<String>>()
                },
                &ParsedAtom::Small(seg),
                res,
            );
            prefix.into_iter().chain(inner).collect()
        },
        hol,
        vec![],
    )
}

fn ro_co<F>([buz, bas, mut dop]: [usize; 3], dug: &F, hol: &ParsedAtom) -> Tape
where
    F: Fn(u128) -> char,
{
    if dop == 0 {
        return vec![];
    }
    let pod = dop - 1;
    let seg = cut(buz, pod, 1, hol); // bloq = buz, start = pod, run = 1
    let mut res = vec!['.'.to_string()];
    res.extend(em_co(
        bas as u128,
        1,
        |_, b, c| {
            std::iter::once(dug(b).to_string())
                .chain(c)
                .collect::<Vec<String>>()
        },
        &seg,
        ro_co([buz, bas, pod], dug, hol),
    ));
    res
}

pub fn number<'src>() -> impl Parser<'src, &'src str, (String, ParsedAtom), Err<'src>> {
    let ud_number = decimal_number().map(|s| ("ud".to_string(), decimal_to_atom(s)));

    let ux_number = hexadecimal_number().map(|s| ("ux".to_string(), hex_to_atom(s)));

    let uc_number = bitcoin_address().try_map(|s, span| {
        let maybe_base58 = base58_to_atom(s);
        match maybe_base58 {
            None => Err(Rich::custom(span, "Invalid BTC address.")),
            Some(atom) => Ok(("uc".to_string(), atom)),
        }
    });

    let ub_number = binary_number().map(|s| ("ub".to_string(), binary_to_atom(s)));

    let uv_number = base32_number().map(|a| ("uv".to_string(), a));

    let uw_number = base64_number().map(|a| ("uw".to_string(), a));

    let ui_number = just("0i")
        .ignore_then(digits())
        .map(|s| ("ui".to_string(), decimal_to_atom(s)));

    let negative = choice((
        hexadecimal_number().map(|s| ("sx".to_string(), hex_to_atom(s))),
        binary_number().map(|s| ("sb".to_string(), binary_to_atom(s))),
        bitcoin_address().try_map(|s, span| {
            let maybe_base58 = base58_to_atom(s);
            match maybe_base58 {
                None => Err(Rich::custom(span, "Invalid BTC address.")),
                Some(atom) => Ok(("uc".to_string(), atom)),
            }
        }),
        base32_number().map(|a| ("sv".to_string(), a)),
        base64_number().map(|a| ("sw".to_string(), a)),
        just("0i")
            .ignore_then(digits())
            .map(|s| ("si".to_string(), decimal_to_atom(s))),
        decimal_number().map(|s| ("sd".to_string(), decimal_to_atom(s))),
    ))
    .boxed();

    let signed_number = // signed: -num and --num
        just('-')
        .ignore_then(
            just('-')
            .ignore_then(negative.clone().map(|(p, q)| (p, apply_sign(true, q))))
            .or(negative.map(|(p, q)| (p, apply_sign(false, q)))));

    choice((
        signed_number, ub_number, uc_number, ui_number, ux_number, uv_number, uw_number, ud_number,
    ))
    .labelled("Number")
}

// decimal without leading 0 and without dots.
//
pub fn decimal_without_leading_zero<'src>() -> impl Parser<'src, &'src str, String, Err<'src>> {
    just('0').to("0".to_string()).or(any()
        .filter(|c: &char| matches!(c, '1'..='9'))
        .then(
            any()
                .filter(|c: &char| c.is_ascii_digit())
                .repeated()
                .collect::<String>(),
        )
        .map(|(h, t)| format!("{h}{t}")))
}

pub fn absolute_date<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let era_year = decimal_without_leading_zero()
        .then(just('-').to(false).or_not().map(|opt| opt.unwrap_or(true)))
        .try_map(|(year_str, era), span| {
            let year: u64 = year_str
                .parse()
                .map_err(|_| Rich::custom(span, "invalid year number"))?;

            if year == 0 {
                return Err(Rich::custom(span, "year must be ≥ 1"));
            }

            Ok((era, year))
        });
    let month = just('.').ignore_then(digits()).try_map(|s: String, span| {
        let m: u64 = s.parse().map_err(|_| Rich::custom(span, "invalid month"))?;
        if (1..=12).contains(&m) {
            Ok(m)
        } else {
            Err(Rich::custom(span, "month out of range (1–12)"))
        }
    });
    let day = just('.').ignore_then(digits()).try_map(|s, span| {
        let d: u64 = s.parse().map_err(|_| Rich::custom(span, "invalid day"))?;
        if (1..=31).contains(&d) {
            Ok(d)
        } else {
            Err(Rich::custom(span, "day out of range (1–31)"))
        }
    });
    let hour_min_secs_fractions = just("..")
        .ignore_then(
            digits()
                .try_map(|s, span| {
                    let h: u64 = s
                        .parse::<u64>()
                        .map_err(|_| Rich::custom(span, "invalid hour"))?;
                    if h < 24 {
                        Ok(h)
                    } else {
                        Err(Rich::custom(span, "hour out of range (0–23)"))
                    }
                })
                .then_ignore(just("."))
                .then(digits().try_map(|s, span| {
                    let m: u64 = s
                        .parse::<u64>()
                        .map_err(|_| Rich::custom(span, "invalid minute"))?;
                    if m < 60 {
                        Ok(m)
                    } else {
                        Err(Rich::custom(span, "minute out of range (0–59)"))
                    }
                }))
                .then_ignore(just("."))
                .then(digits().try_map(|s, span| {
                    let s: u64 = s
                        .parse::<u64>()
                        .map_err(|_| Rich::custom(span, "invalid second"))?;
                    if s < 60 {
                        Ok(s)
                    } else {
                        Err(Rich::custom(span, "second out of range (0–59)"))
                    }
                })),
        )
        .then(
            just("..")
                .ignore_then(
                    alphanumeric()
                        .separated_by(just("."))
                        .at_least(1)
                        .collect::<Vec<String>>(),
                )
                .or_not()
                .map(|opt| opt.unwrap_or_default()),
        )
        .try_map(|(((h, m), s), frags), span| {
            let mut fractions = Vec::new();

            for f in frags {
                let val = u16::from_str_radix(&f, 16)
                    .map_err(|_| Rich::custom(span, "invalid fraction digits"))?;
                fractions.push(val);
            }

            Ok((h, m, s, fractions))
        })
        .or_not()
        .map(|opt| opt.unwrap_or((0, 0, 0, Vec::new())));

    era_year
        .then(month)
        .then(day)
        .then(hour_min_secs_fractions)
        .map(|((((era, y), m), d), (hour, min, sec, f))| {
            ParsedAtom::Small(year(era, y, m, d, hour, min, sec, &f))
        })
}

fn unit_value_pair<'src>() -> impl Parser<'src, &'src str, (char, u64), Err<'src>> {
    one_of("dhms").then(decimal_without_leading_zero().try_map(|s, span| {
        s.parse::<u64>()
            .map_err(|_| Rich::custom(span, "Invalid Number"))
    }))
}

pub fn relative_date<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let time_part = unit_value_pair()
        .separated_by(just('.'))
        .at_least(1)
        .collect::<Vec<(char, u64)>>();

    let hex_part = just("..")
        .ignore_then(
            any()
                .filter(|c: &char| c.is_ascii_hexdigit())
                .repeated()
                .exactly(4)
                .collect::<String>()
                .map(|s| u16::from_str_radix(&s, 16).unwrap_or(0))
                .separated_by(just('.'))
                .at_least(1)
                .collect::<Vec<u16>>(),
        )
        .or_not()
        .map(|v| v.unwrap_or_default());

    time_part
        .then(hex_part)
        .map(|(pairs, hex_vec): (Vec<(char, u64)>, Vec<u16>)| {
            let mut days = 0u64;
            let mut hours = 0u64;
            let mut minutes = 0u64;
            let mut seconds = 0u64;

            for (unit, value) in pairs {
                match unit {
                    'd' => days += value,
                    'h' => hours += value,
                    'm' => minutes += value,
                    's' => seconds += value,
                    _ => {}
                }
            }

            ParsedAtom::Small(yule(days, hours, minutes, seconds, &hex_vec))
        })
}

// ++year: date -> @da
pub fn year(a: bool, y: u64, m: u64, d: u64, h: u64, min: u64, s: u64, f: &[u16]) -> u128 {
    let yer = if a {
        YEAR_OFFSET + y
    } else {
        // (sub 292.277.024.400 (dec y))
        YEAR_OFFSET - (y - 1)
    };

    let day_count = yawn(yer, m, d);

    yule(day_count, h, min, s, f)
}

pub fn yell(now: &ParsedAtom) -> Tarp {
    let sec_atom = rsh(6, 1, now);

    let raw = end(6, 1, now);

    let mut fan = Vec::new();
    let mut muc = 4;
    let mut current_raw = raw.clone();

    while muc > 0 && !current_raw.is_zero() {
        muc -= 1;
        let digit_atom = cut(4, muc, 1, &current_raw);
        let digit: u64 = match &digit_atom {
            ParsedAtom::Small(x) => *x as u64,
            ParsedAtom::Big(b) => b.clone().try_into().unwrap_or(0),
        };
        fan.push(digit);

        current_raw = end(4, muc, &current_raw);
    }

    let sec_u64: u64 = match &sec_atom {
        ParsedAtom::Small(x) => *x as u64,
        ParsedAtom::Big(b) => b.clone().try_into().expect("yell: sec too large"),
    };

    let day = (sec_u64 / DAY) as u64;
    let sec = (sec_u64 % DAY) as u64;
    let hor = (sec / HOR) as u64;
    let sec = (sec % HOR) as u64;
    let mit = (sec / MIT) as u64;
    let sec = (sec % MIT) as u64;

    Tarp {
        d: day,
        h: hor,
        m: mit,
        s: sec,
        f: fan,
    }
}

pub fn yore(now: &ParsedAtom) -> Date {
    let rip: Tarp = yell(now);
    let (y_ger, m_ger, d_ger) = yall(rip.d);

    const PIVOT: u64 = 292_277_024_400;

    let (era, y_out) = if y_ger > PIVOT {
        (true, y_ger - PIVOT)
    } else {
        (false, PIVOT - y_ger)
    };

    Date {
        era,
        y: y_out,
        m: m_ger,
        t: Tarp {
            d: d_ger,
            h: rip.h,
            m: rip.m,
            s: rip.s,
            f: rip.f,
        },
    }
}

pub fn yall(day: u64) -> (u64, u64, u64) {
    let mut day = day;
    let mut era = 0;
    let mut cet = 0;
    let mut lep = false;

    // => .(era (div day era:yo), day (mod day era:yo))
    era = day / ERA;
    day %= ERA;

    // ?: (lth day +(cet:yo)) ...
    if day < CETY + 1 {
        lep = true;
        cet = 0;
    } else {
        lep = false;
        day = day - (CETY + 1);
        cet = 1 + (day / CETY);
        day %= CETY;
    }

    let mut yer = 400 * era + 100 * cet;

    // |- loop: subtract years
    loop {
        let dis = if lep { 366 } else { 365 };
        if day < dis {
            break;
        }
        let ner = yer + 1;
        day = day - dis;
        // lep =(0 (end [0 2] ner)) → is ner divisible by 4? (end [0 2] = lowest 2 bits)
        // end(0, 2, ner) = lowest 2 bits; =0 means divisible by 4
        lep = (ner & 3) == 0; // faster than atom ops
        yer = ner;
    }

    // month loop
    let cah = if lep { &MOY } else { &MOH };
    let mut mot = 0;
    loop {
        let zis = cah[mot as usize];
        if day < zis {
            return (yer, mot + 1, day + 1); // 1-based month/day
        }
        day -= zis;
        mot += 1;
    }
}

fn is_leap(year: u64) -> bool {
    (year % 4 == 0) && (year % 100 != 0 || year % 400 == 0)
}

pub fn is_leap_year(year: i32) -> bool {
    // Gregorian calendar proleptic
    (year % 4 == 0) && (year % 100 != 0 || year % 400 == 0)
}

pub fn yule(d: u64, h: u64, m: u64, s: u64, f: &[u16]) -> u128 {
    let sec = d * DAY + h * HOR + m * MIT + s;

    let mut fac: u64 = 0;
    let mut muc = 4i32; // starts at 4
    for &val in f.iter().take(4) {
        muc -= 1; // decrement *before* shift
        fac += (val as u64) << (muc as u32 * 16);
    }

    ((sec as u128) << 64) | (fac as u128)
}

fn bloq_bits(bloq: u32) -> u32 {
    if bloq >= 7 {
        panic!("bloq must be < 7 (max 64-bit chunks for u128)");
    }
    1 << bloq
}

pub fn met(bloq: usize, atom: &ParsedAtom) -> usize {
    let bits_per_block: usize = 1usize << bloq;

    match atom {
        ParsedAtom::Small(n) => {
            if *n == 0 {
                1
            } else {
                let atom_bits: usize = 128 - n.leading_zeros() as usize;
                (atom_bits + bits_per_block - 1) / bits_per_block
            }
        }
        ParsedAtom::Big(b) => {
            if b.is_zero() {
                1
            } else {
                let atom_bits: usize = b.bits() as usize;
                (atom_bits + bits_per_block - 1) / bits_per_block
            }
        }
    }
}

/// rep: assemble list of ParsedAtoms into one ParsedAtom using bite spec
///
/// - `bloq`: block size exponent (e.g. 3 → 8-bit blocks)
/// - `step_opt`: number of bloqs to take from each atom; if `None`, defaults to 1 (per Hoon ?^(a a [a *step]))
/// - `list`: slice of ParsedAtoms (representing Hoon `(list @)`)
///
/// Semantics:
///   result = Σ_i ( (atom_i & mask) << (i * chunk_bits) )
///   where mask = (1 << chunk_bits) - 1
pub fn rep(bloq: usize, step_opt: Option<usize>, list: &[ParsedAtom]) -> ParsedAtom {
    let step = step_opt.unwrap_or(1); // default step = 1

    let bloq_size = 1usize << bloq; // 2^bloq
    let chunk_bits = step * bloq_size; // bits per item

    if list.is_empty() || chunk_bits == 0 {
        return ParsedAtom::Small(0);
    }

    let mut result = BigUint::from(0u32);

    for (i, atom) in list.iter().enumerate() {
        let atom_bu = atom.to_biguint();

        let truncated = if chunk_bits < 128 {
            let mask = (1u128 << chunk_bits) - 1;
            let mask_bu = BigUint::from(mask);
            atom_bu & mask_bu
        } else {
            if atom_bu.bits() as usize <= chunk_bits {
                atom_bu
            } else {
                let mask = (BigUint::from(1u32) << chunk_bits) - 1u8;
                &atom_bu & mask
            }
        };

        let shifted = if i == 0 {
            truncated
        } else {
            truncated << (i * chunk_bits)
        };

        result += shifted;
    }

    ParsedAtom::Big(result)
}

pub fn rap(bloq: usize, chunks: &[u128]) -> ParsedAtom {
    if chunks.is_empty() {
        return ParsedAtom::Small(0);
    }

    let bits_per_bloq = bloq_bits(bloq as u32) as u64;
    let mut result = BigUint::zero();
    let mut shift = 0u64;

    for &chunk in chunks {
        let width_bloqs = met(bloq, &ParsedAtom::Small(chunk)) as u64;
        let width_bits = width_bloqs * bits_per_bloq;

        let mask = if width_bits >= 128 {
            u128::MAX
        } else {
            (1u128 << width_bits) - 1
        };
        if chunk & !mask != 0 {
            panic!("atom {:#x} too large for bloq {}", chunk, bloq);
        }

        let chunk_big = BigUint::from(chunk);
        result |= chunk_big << shift;

        shift += width_bits;

        if shift > 128 {}
    }

    // Now decide which variant to return
    if shift <= 128 {
        let value = result
            .to_u128()
            .expect("logic error: shift <=128 but not u128");
        ParsedAtom::Small(value)
    } else {
        ParsedAtom::Big(result)
    }
}

fn cut_u(v: u128, shift: usize, bits: usize) -> u8 {
    ((v >> shift) & ((1 << bits) - 1)) as u8
}

/// Extract `run` bloqs starting at bloq `start`, where each bloq is `2^bloq` bits.
pub fn cut(bloq: usize, start: usize, run: usize, atom: &ParsedAtom) -> ParsedAtom {
    if run == 0 {
        return ParsedAtom::Small(0);
    }

    let bloq_bits = match 1usize.checked_shl(bloq as u32) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };

    let bit_start = match start.checked_mul(bloq_bits) {
        Some(s) => s,
        None => return ParsedAtom::Small(0),
    };

    let bit_len = match run.checked_mul(bloq_bits) {
        Some(l) => l,
        None => return ParsedAtom::Small(0),
    };

    let src_bits = match atom {
        ParsedAtom::Small(0) => 0,
        ParsedAtom::Small(n) => (128 - n.leading_zeros()) as usize,
        ParsedAtom::Big(b) => b.bits() as usize,
    };

    if bit_start >= src_bits {
        return ParsedAtom::Small(0);
    }

    let bit_len = cmp::min(bit_len, src_bits - bit_start);
    if bit_len == 0 {
        return ParsedAtom::Small(0);
    }

    let shifted = match atom {
        ParsedAtom::Small(n) => {
            if bit_start >= 128 {
                ParsedAtom::Small(0)
            } else {
                ParsedAtom::Small(n >> bit_start)
            }
        }
        ParsedAtom::Big(b) => {
            if bit_start == 0 {
                atom.clone()
            } else {
                ParsedAtom::from_biguint(b >> bit_start)
            }
        }
    };

    match &shifted {
        ParsedAtom::Small(n) => {
            if bit_len >= 128 {
                shifted
            } else {
                let mask = (1u128 << bit_len) - 1;
                ParsedAtom::Small(*n & mask)
            }
        }
        ParsedAtom::Big(b) => {
            // b: &BigUint
            if bit_len <= 128 {
                // Extract low 128 bits manually (portable)
                let low_u128 = {
                    // Convert to u128, but preserve low bits even if truncated
                    // u128::try_from returns Err for >u128::MAX, but we want modulo 2^128
                    // So: take first 2 u64 limbs
                    let mut limbs = b.iter_u64_digits();
                    let lo = limbs.next().unwrap_or(0);
                    let hi = limbs.next().unwrap_or(0);
                    ((hi as u128) << 64) | (lo as u128)
                };
                let mask = if bit_len == 128 {
                    u128::MAX
                } else {
                    (1u128 << bit_len) - 1
                };
                ParsedAtom::Small(low_u128 & mask)
            } else {
                // Big mask: (1 << bit_len) - 1
                let mask = (BigUint::one() << bit_len) - BigUint::one();
                // Use & with references to avoid move
                let masked = b & &mask; // &BigUint & &BigUint → BigUint
                ParsedAtom::from_biguint(masked)
            }
        }
    }
}

pub fn lsh(bloq: usize, step: usize, atom: &ParsedAtom) -> ParsedAtom {
    let bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };
    atom_shl(atom, bits)
}

pub fn rsh(bloq: usize, step: usize, atom: &ParsedAtom) -> ParsedAtom {
    let bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };
    atom_shr(atom, bits)
}

fn lsh_u128(bloq: usize, step: usize, atom: u128) -> u128 {
    let bits = step.checked_mul(1 << bloq).unwrap_or(128);
    if bits >= 128 {
        0
    } else {
        atom << bits
    }
}

fn rsh_u128(bloq: usize, step: usize, atom: u128) -> u128 {
    let bits = step.checked_mul(1 << bloq).unwrap_or(128);
    if bits >= 128 {
        0
    } else {
        atom >> bits
    }
}

fn lsh_big(bloq: usize, step: usize, atom: &BigUint) -> BigUint {
    let bits = step.checked_mul(1 << bloq).unwrap_or(usize::MAX);
    if bits == 0 {
        atom.clone()
    } else {
        atom << bits
    }
}

fn rsh_big(bloq: usize, step: usize, atom: &BigUint) -> BigUint {
    let bits = step.checked_mul(1 << bloq).unwrap_or(usize::MAX);
    if bits == 0 {
        atom.clone()
    } else {
        atom >> bits
    }
}

fn end(bloq: usize, step: usize, atom: &ParsedAtom) -> ParsedAtom {
    let total_bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b,
        None => return ParsedAtom::Small(0),
    };
    atom_mask_low_bits(atom, total_bits)
}

fn end_big(bloq: usize, step: usize, atom: &BigUint) -> BigUint {
    let total_bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b as u128,
        None => return BigUint::zero(),
    };
    if total_bits == 0 {
        return BigUint::zero();
    }
    let mask = (BigUint::one() << total_bits) - BigUint::one();
    atom & &mask
}

fn end_u128(bloq: usize, step: usize, atom: u128) -> u128 {
    let total_bits = match step.checked_mul(1usize << bloq) {
        Some(b) => b as u128,
        None => return 0,
    };
    if total_bits >= 128 {
        atom
    } else {
        let mask = (1u128 << total_bits) - 1;
        atom & mask
    }
}

pub const SIS: [[u8; 3]; 256] = [
    *b"doz", *b"mar", *b"bin", *b"wan", *b"sam", *b"lit", *b"sig", *b"hid", *b"fid", *b"lis",
    *b"sog", *b"dir", *b"wac", *b"sab", *b"wis", *b"sib", *b"rig", *b"sol", *b"dop", *b"mod",
    *b"fog", *b"lid", *b"hop", *b"dar", *b"dor", *b"lor", *b"hod", *b"fol", *b"rin", *b"tog",
    *b"sil", *b"mir", *b"hol", *b"pas", *b"lac", *b"rov", *b"liv", *b"dal", *b"sat", *b"lib",
    *b"tab", *b"han", *b"tic", *b"pid", *b"tor", *b"bol", *b"fos", *b"dot", *b"los", *b"dil",
    *b"for", *b"pil", *b"ram", *b"tir", *b"win", *b"tad", *b"bic", *b"dif", *b"roc", *b"wid",
    *b"bis", *b"das", *b"mid", *b"lop", *b"ril", *b"nar", *b"dap", *b"mol", *b"san", *b"loc",
    *b"nov", *b"sit", *b"nid", *b"tip", *b"sic", *b"rop", *b"wit", *b"nat", *b"pan", *b"min",
    *b"rit", *b"pod", *b"mot", *b"tam", *b"tol", *b"sav", *b"pos", *b"nap", *b"nop", *b"som",
    *b"fin", *b"fon", *b"ban", *b"mor", *b"wor", *b"sip", *b"ron", *b"nor", *b"bot", *b"wic",
    *b"soc", *b"wat", *b"dol", *b"mag", *b"pic", *b"dav", *b"bid", *b"bal", *b"tim", *b"tas",
    *b"mal", *b"lig", *b"siv", *b"tag", *b"pad", *b"sal", *b"div", *b"dac", *b"tan", *b"sid",
    *b"fab", *b"tar", *b"mon", *b"ran", *b"nis", *b"wol", *b"mis", *b"pal", *b"las", *b"dis",
    *b"map", *b"rab", *b"tob", *b"rol", *b"lat", *b"lon", *b"nod", *b"nav", *b"fig", *b"nom",
    *b"nib", *b"pag", *b"sop", *b"ral", *b"bil", *b"had", *b"doc", *b"rid", *b"moc", *b"pac",
    *b"rav", *b"rip", *b"fal", *b"tod", *b"til", *b"tin", *b"hap", *b"mic", *b"fan", *b"pat",
    *b"tac", *b"lab", *b"mog", *b"sim", *b"son", *b"pin", *b"lom", *b"ric", *b"tap", *b"fir",
    *b"has", *b"bos", *b"bat", *b"poc", *b"hac", *b"tid", *b"hav", *b"sap", *b"lin", *b"dib",
    *b"hos", *b"dab", *b"bit", *b"bar", *b"rac", *b"par", *b"lod", *b"dos", *b"bor", *b"toc",
    *b"hil", *b"mac", *b"tom", *b"dig", *b"fil", *b"fas", *b"mit", *b"hob", *b"har", *b"mig",
    *b"hin", *b"rad", *b"mas", *b"hal", *b"rag", *b"lag", *b"fad", *b"top", *b"mop", *b"hab",
    *b"nil", *b"nos", *b"mil", *b"fop", *b"fam", *b"dat", *b"nol", *b"din", *b"hat", *b"nac",
    *b"ris", *b"fot", *b"rib", *b"hoc", *b"nim", *b"lar", *b"fit", *b"wal", *b"rap", *b"sar",
    *b"nal", *b"mos", *b"lan", *b"don", *b"dan", *b"lad", *b"dov", *b"riv", *b"bac", *b"pol",
    *b"lap", *b"tal", *b"pit", *b"nam", *b"bon", *b"ros", *b"ton", *b"fod", *b"pon", *b"sov",
    *b"noc", *b"sor", *b"lav", *b"mat", *b"mip", *b"fip",
];

pub const DEX: [[u8; 3]; 256] = [
    *b"zod", *b"nec", *b"bud", *b"wes", *b"sev", *b"per", *b"sut", *b"let", *b"ful", *b"pen",
    *b"syt", *b"dur", *b"wep", *b"ser", *b"wyl", *b"sun", *b"ryp", *b"syx", *b"dyr", *b"nup",
    *b"heb", *b"peg", *b"lup", *b"dep", *b"dys", *b"put", *b"lug", *b"hec", *b"ryt", *b"tyv",
    *b"syd", *b"nex", *b"lun", *b"mep", *b"lut", *b"sep", *b"pes", *b"del", *b"sul", *b"ped",
    *b"tem", *b"led", *b"tul", *b"met", *b"wen", *b"byn", *b"hex", *b"feb", *b"pyl", *b"dul",
    *b"het", *b"mev", *b"rut", *b"tyl", *b"wyd", *b"tep", *b"bes", *b"dex", *b"sef", *b"wyc",
    *b"bur", *b"der", *b"nep", *b"pur", *b"rys", *b"reb", *b"den", *b"nut", *b"sub", *b"pet",
    *b"rul", *b"syn", *b"reg", *b"tyd", *b"sup", *b"sem", *b"wyn", *b"rec", *b"meg", *b"net",
    *b"sec", *b"mul", *b"nym", *b"tev", *b"web", *b"sum", *b"mut", *b"nyx", *b"rex", *b"teb",
    *b"fus", *b"hep", *b"ben", *b"mus", *b"wyx", *b"sym", *b"sel", *b"ruc", *b"dec", *b"wex",
    *b"syr", *b"wet", *b"dyl", *b"myn", *b"mes", *b"det", *b"bet", *b"bel", *b"tux", *b"tug",
    *b"myr", *b"pel", *b"syp", *b"ter", *b"meb", *b"set", *b"dut", *b"deg", *b"tex", *b"sur",
    *b"fel", *b"tud", *b"nux", *b"rux", *b"ren", *b"wyt", *b"nub", *b"med", *b"lyt", *b"dus",
    *b"neb", *b"rum", *b"tyn", *b"seg", *b"lyx", *b"pun", *b"res", *b"red", *b"fun", *b"rev",
    *b"ref", *b"mec", *b"ted", *b"rus", *b"bex", *b"leb", *b"dux", *b"ryn", *b"num", *b"pyx",
    *b"ryg", *b"ryx", *b"fep", *b"tyr", *b"tus", *b"tyc", *b"leg", *b"nem", *b"fer", *b"mer",
    *b"ten", *b"lus", *b"nus", *b"syl", *b"tec", *b"mex", *b"pub", *b"rym", *b"tuc", *b"fyl",
    *b"lep", *b"deb", *b"ber", *b"mug", *b"hut", *b"tun", *b"byl", *b"sud", *b"pem", *b"dev",
    *b"lur", *b"def", *b"bus", *b"bep", *b"run", *b"mel", *b"pex", *b"dyt", *b"byt", *b"typ",
    *b"lev", *b"myl", *b"wed", *b"duc", *b"fur", *b"fex", *b"nul", *b"luc", *b"len", *b"ner",
    *b"lex", *b"rup", *b"ned", *b"lec", *b"ryd", *b"lyd", *b"fen", *b"wel", *b"nyd", *b"hus",
    *b"rel", *b"rud", *b"nes", *b"hes", *b"fet", *b"des", *b"ret", *b"dun", *b"ler", *b"nyr",
    *b"seb", *b"hul", *b"ryl", *b"lud", *b"rem", *b"lys", *b"fyn", *b"wer", *b"ryc", *b"sug",
    *b"nys", *b"nyl", *b"lyn", *b"dyn", *b"dem", *b"lux", *b"fed", *b"sed", *b"bec", *b"mun",
    *b"lyr", *b"tes", *b"mud", *b"nyt", *b"byr", *b"sen", *b"weg", *b"fyr", *b"mur", *b"tel",
    *b"rep", *b"teg", *b"pec", *b"nel", *b"nev", *b"fes",
];

/// Fetch prefix syllable (Hoon ++tos)
pub fn tos_po(i: u8) -> ParsedAtom {
    let b = SIS[i as usize];
    ParsedAtom::Small((b[0] as u128) | ((b[1] as u128) << 8) | ((b[2] as u128) << 16))
}

/// Fetch suffix syllable (Hoon ++tod)
pub fn tod_po(i: u8) -> ParsedAtom {
    let b = DEX[i as usize];
    ParsedAtom::Small((b[0] as u128) | ((b[1] as u128) << 8) | ((b[2] as u128) << 16))
}

/// Linear prefix search (Hoon ++ins)
pub fn ins(a: &[u8]) -> Option<u8> {
    if a.len() != 3 {
        return None;
    }

    let key = [a[0], a[1], a[2]];

    for (i, entry) in SIS.iter().enumerate() {
        if *entry == key {
            return Some(i as u8);
        }
    }

    None
}

/// Linear suffix search (Hoon ++ind)
pub fn ind(a: &[u8]) -> Option<u8> {
    if a.len() != 3 {
        return None;
    }

    let key = [a[0], a[1], a[2]];

    for (i, entry) in DEX.iter().enumerate() {
        if *entry == key {
            return Some(i as u8);
        }
    }

    None
}

// +tip:ab
pub fn tip<'src>() -> impl Parser<'src, &'src str, u8, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .repeated()
        .exactly(3)
        .collect::<String>()
        .try_map(|s, span| match ins(s.as_bytes()) {
            Some(i) => Ok(i),
            None => Err(Rich::custom(span, format!("invalid prefix syllable '{s}'"))),
        })
        .labelled("Phonetic Prefix")
}

// +tiq:ab
pub fn tiq<'src>() -> impl Parser<'src, &'src str, u8, Err<'src>> {
    any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .repeated()
        .exactly(3)
        .collect::<String>()
        .try_map(|s, span| match ind(s.as_bytes()) {
            Some(i) => Ok(i),
            None => Err(Rich::custom(span, format!("invalid suffix syllable '{s}'"))),
        })
        .labelled("Phonetic Suffix")
}

// +hif:ab
pub fn hif<'src>() -> impl Parser<'src, &'src str, u16, Err<'src>> {
    tip()
        .then(tiq())
        .try_map(|(p, q), span| Ok((p as u16) * 256 + (q as u16)))
}

pub fn phonemic_name<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    let tep = any()
        .filter(|c: &char| c.is_ascii_lowercase())
        .repeated()
        .exactly(3)
        .to_slice()
        .try_map(|s: &str, span| {
            if s == "doz" {
                return Err(Rich::custom(span, "prefix 'doz' is forbidden"));
            }
            match ins(s.as_bytes()) {
                Some(i) => Ok(i),
                None => Err(Rich::custom(span, format!("invalid prefix syllable '{s}'"))),
            }
        })
        .labelled("Phonetic Prefix");
    let hef = tip()
        .then(tiq())
        .try_map(|(p, q), span| {
            let val = (p as u16) * 256 + (q as u16);
            if val == 0 {
                Err(Rich::custom(span, format!("phonetic is zero")))
            } else {
                Ok(val)
            }
        })
        .boxed();
    let huf = hef
        .clone() // u16
        .then(
            just('-')
                .ignore_then(hif()) // u16
                .repeated()
                .at_most(3)
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect::<Vec<_>>())
        .map(|hefs: Vec<u16>| {
            let mut acc = BigUint::from(0u32);
            for &digit in &hefs {
                acc = (acc << 16) + BigUint::from(digit);
            }
            acc
        });
    let hyf = hif()
        .separated_by(just('-'))
        .exactly(4)
        .collect::<Vec<_>>()
        .map(|hefs: Vec<u16>| {
            let mut acc = BigUint::from(0u32);
            for &digit in &hefs {
                acc = (acc << 16) + BigUint::from(digit);
            }
            acc
        });
    let other = huf
        .then(
            just("--")
                .ignore_then(gap().or_not())
                .ignore_then(hyf)
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect::<Vec<_>>())
        .map(|hefs: Vec<BigUint>| {
            let acc = hefs
                .iter()
                .fold(BigUint::from(0u32), |acc, d| (acc << 64) + d);
            ParsedAtom::Big(fynd_big(&acc))
        });
    let planet_moon = hef
        .then(
            just('-')
                .ignore_then(hif())
                .repeated()
                .at_least(1)
                .at_most(3)
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect::<Vec<_>>())
        .map(|hefs: Vec<u16>| {
            let mut acc = BigUint::from_u32(0).unwrap();
            for &digit in &hefs {
                acc = (acc << 16) + BigUint::from_u32(digit as u32).unwrap();
            }
            ParsedAtom::Big(fynd_big(&acc))
        });
    let star = tep.then(tiq()).try_map(|(p, q), span| {
        let x = (p as u16) * 256 + (q as u16);
        Ok(ParsedAtom::Small(x as u128))
    });
    let galaxy = tiq().map(|p| ParsedAtom::Small(p.into()));

    choice((
        other.labelled("Long Phonemic"),
        planet_moon.labelled("Planet or Moon"),
        star.labelled("Star"),
        galaxy.labelled("Galaxy"),
    ))
}

pub fn phonemic_name_unscrambled<'src>() -> impl Parser<'src, &'src str, ParsedAtom, Err<'src>> {
    hif()
        .or(tiq().map(|i| i as u16))
        .then(
            just('-')
                .ignore_then(gap().or_not())
                .ignore_then(hif())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| {
            std::iter::once(first)
                .chain(rest)
                .map(ParsedAtom::from)
                .collect::<Vec<ParsedAtom>>()
        })
        .map(|mut hifs| {
            hifs.reverse();
            rep(4, None, &hifs)
        })
}

fn dis_big(x: &BigUint, mask: &BigUint) -> BigUint {
    x & mask
}

// fn dis(x: u64, mask: u64) -> u64 {
fn dis<T: Copy + BitAnd<Output = T>>(x: T, mask: T) -> T {
    x & mask
}

fn con(hi: u64, lo: u64) -> u64 {
    hi | lo
}

fn con_atoms(hi: ParsedAtom, lo: ParsedAtom) -> ParsedAtom {
    match (hi, lo) {
        (ParsedAtom::Small(a), ParsedAtom::Small(b)) => ParsedAtom::Small(a | b),
        (a, b) => {
            let x = a.to_biguint();
            let y = b.to_biguint();
            ParsedAtom::from_biguint(x | y)
        }
    }
}

fn mix(x: u64, y: u64) -> u64 {
    x ^ y
}

fn mix_big(x: &BigUint, y: &BigUint) -> BigUint {
    x ^ y
}

fn mix_atoms(a: ParsedAtom, b: ParsedAtom) -> ParsedAtom {
    match (a, b) {
        (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x ^ y),
        (a, b) => {
            let x = a.to_biguint();
            let y = b.to_biguint();
            ParsedAtom::from_biguint(&x ^ &y)
        }
    }
}

const RAKU: [u32; 4] = [0xb76d_5eed, 0xee28_1300, 0x85bc_ae01, 0x4b38_7af7];
#[inline]
fn rol32(x: u32, r: u32) -> u32 {
    x.rotate_left(r)
}

#[inline]
fn fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h
}

fn muk(seed: u32, len: u32, key: u64) -> u32 {
    let c1: u32 = 0xcc9e_2d51;
    let c2: u32 = 0x1b87_3593;

    let mut data = vec![0u8; len as usize];
    let mut k = key;
    for i in 0..len as usize {
        data[i] = (k & 0xff) as u8;
        k >>= 8;
    }

    let nblocks = (len / 4) as usize; // intentionally off-by-one
    let mut h1 = seed;

    let mut blocks = Vec::new();
    for i in 0..nblocks {
        let mut v = 0u32;
        for j in 0..4 {
            let idx = i * 4 + j;
            if idx < data.len() {
                v |= (data[idx] as u32) << (8 * j);
            }
        }
        blocks.push(v);
    }

    let mut i = nblocks;
    while i > 0 {
        let mut k1 = blocks[nblocks - i];
        k1 = k1.wrapping_mul(c1);
        k1 = k1.rotate_left(15);
        k1 = k1.wrapping_mul(c2);

        h1 ^= k1;
        h1 = h1.rotate_left(13);
        h1 = h1.wrapping_mul(5).wrapping_add(0xe654_6b64);
        i -= 1;
    }

    let tail = &data[(nblocks * 4)..];
    let mut k1 = 0u32;

    match len & 3 {
        3 => {
            k1 ^= (tail[2] as u32) << 16;
            k1 ^= (tail[1] as u32) << 8;
            k1 ^= tail[0] as u32;
            k1 = k1.wrapping_mul(c1);
            k1 = k1.rotate_left(15);
            k1 = k1.wrapping_mul(c2);
            h1 ^= k1;
        }
        2 => {
            k1 ^= (tail[1] as u32) << 8;
            k1 ^= tail[0] as u32;
            k1 = k1.wrapping_mul(c1);
            k1 = k1.rotate_left(15);
            k1 = k1.wrapping_mul(c2);
            h1 ^= k1;
        }
        1 => {
            k1 ^= tail[0] as u32;
            k1 = k1.wrapping_mul(c1);
            k1 = k1.rotate_left(15);
            k1 = k1.wrapping_mul(c2);
            h1 ^= k1;
        }
        _ => {}
    }

    h1 ^= len;
    fmix32(h1)
}

fn eff(j: u64, r: u64) -> u64 {
    let seed = RAKU[(j as usize) & 3];
    muk(seed, 2, r) as u64
}

fn fen(r: u64, a: u64, b: u64, m: u64) -> u64 {
    let mut j = r;

    let (ahh, ale) = if r % 2 == 0 {
        (m % a, m / a)
    } else {
        (m / a, m % a)
    };

    let (mut ell, mut arr) = if ale == a { (ahh, ale) } else { (ale, ahh) };

    while j >= 1 {
        let f = eff(j - 1, ell);

        let tmp = if j % 2 != 0 {
            (arr + a - (f % a)) % a
        } else {
            (arr + b - (f % b)) % b
        };

        j -= 1;
        arr = ell;
        ell = tmp;
    }

    &arr * a + ell
}

fn fe<F>(r: u64, a: &ParsedAtom, b: &ParsedAtom, prf: &F, m: &ParsedAtom) -> ParsedAtom
where
    F: Fn(u64, &ParsedAtom) -> ParsedAtom,
{
    let mut j: u64 = 1;
    let mut ell = end(0, met(0, a), m); // m mod a = lowest (bitlen a) bits of m
    let mut arr = rsh(0, met(0, a), m); // m div a = m >> (bitlen a)

    loop {
        if j > r {
            if r % 2 == 1 {
                let shifted = match &arr {
                    ParsedAtom::Small(n) => {
                        let shifted_n = n.checked_shl(16).unwrap_or(0);
                        ParsedAtom::Small(shifted_n) | ell.clone()
                    }
                    ParsedAtom::Big(big) => {
                        let shifted = (big.clone() << 16) + ell.to_biguint();
                        ParsedAtom::Big(shifted)
                    }
                };
                return shifted;
            } else {
                // even rounds
                if arr.eq(a) {
                    let a_bits = met(0, a);
                    let shifted_a = rsh(0, 0, a); // identity
                    let shifted = match a {
                        ParsedAtom::Small(n) => {
                            let shifted_n = n.checked_shl(a_bits as u32).unwrap_or(0);
                            ParsedAtom::Small(shifted_n) | ell.clone()
                        }
                        ParsedAtom::Big(big) => {
                            let shifted = (big.clone() << a_bits) + ell.to_biguint();
                            ParsedAtom::Big(shifted)
                        }
                    };
                    return shifted;
                } else {
                    let a_bits = met(0, a);
                    let shifted = match &ell {
                        ParsedAtom::Small(n) => {
                            let shifted_n = n.checked_shl(a_bits as u32).unwrap_or(0);
                            ParsedAtom::Small(shifted_n) | arr.clone()
                        }
                        ParsedAtom::Big(big) => {
                            let shifted = (big.clone() << a_bits) + arr.to_biguint();
                            ParsedAtom::Big(shifted)
                        }
                    };
                    return shifted;
                }
            }
        }

        let f = prf(j - 1, &arr);

        let modulus = if j % 2 == 1 { a } else { b };
        let sum = match (&f, &ell) {
            (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x.wrapping_add(*y)),
            _ => {
                let bx = f.to_biguint();
                let by = ell.to_biguint();
                ParsedAtom::Big(&bx + &by)
            }
        };
        let tmp = end(0, met(0, modulus), &sum); // sum mod modulus

        ell = arr;
        arr = tmp;
        j += 1;
    }
}

pub fn feis(m: ParsedAtom) -> ParsedAtom {
    debug_assert!(m.lt(&ParsedAtom::Small(0xffff_0000))); // domain guarantee
    let m_u64 = m.to_u64_lossy();
    let a = 0xffffu64;
    let b = 0x1_0000u64;
    let k = a * b; // 0xffff_0000

    let mut c = fe_u64(4, a, b, |j, r| eff(j, r), m_u64);
    while c >= k {
        c = fe_u64(4, a, b, |j, r| eff(j, r), c);
    }
    ParsedAtom::Small(c as u128)
}

fn fe_u64(r: u64, a: u64, b: u64, prf: impl Fn(u64, u64) -> u64, m: u64) -> u64 {
    let mut j = 1u64;
    let mut ell = m % a;
    let mut arr = m / a;

    loop {
        if j > r {
            return if r % 2 == 1 {
                arr * a + ell
            } else if arr == a {
                arr * a + ell
            } else {
                ell * a + arr
            };
        }

        let f = prf(j - 1, arr);
        let tmp = if j % 2 == 1 {
            (f + ell) % a
        } else {
            (f + ell) % b
        };

        ell = arr;
        arr = tmp;
        j += 1;
    }
}

fn feen(r: u64, a: u64, b: u64, k: u64, m: u64) -> u64 {
    let c = fen(r, a, b, m);
    if c < k.into() {
        c
    } else {
        fen(r, a, b, c)
    }
}

pub fn fein(pyn: ParsedAtom) -> ParsedAtom {
    let lower_16 = ParsedAtom::Small(0x1_0000);
    let upper_16 = ParsedAtom::Small(0xffff_ffff);
    let lower_32 = ParsedAtom::Small(0x1_0000_0000);
    let upper_32 = ParsedAtom::Small(0xffff_ffff_ffff_ffff);

    if pyn.ge(&lower_16) && pyn.le(&upper_16) {
        let offset = match (&pyn, &lower_16) {
            (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x - y),
            _ => ParsedAtom::Big(&pyn.to_biguint() - &lower_16.to_biguint()),
        };
        let feised = feis(offset);
        match (&feised, &lower_16) {
            (ParsedAtom::Small(x), ParsedAtom::Small(y)) => ParsedAtom::Small(x + y),
            _ => ParsedAtom::Big(&feised.to_biguint() + &lower_16.to_biguint()),
        }
    } else if pyn.ge(&lower_32) && pyn.le(&upper_32) {
        let mask_lo = ParsedAtom::Small(0xffff_ffff);
        let lo = match (&pyn, &mask_lo) {
            (ParsedAtom::Small(x), ParsedAtom::Small(m)) => ParsedAtom::Small(dis(*x, *m)),
            _ => ParsedAtom::Big(dis_big(&pyn.to_biguint(), &mask_lo.to_biguint())),
        };

        let mask_hi = ParsedAtom::Small(0xffff_ffff_0000_0000);
        let hi = match (&pyn, &mask_hi) {
            (ParsedAtom::Small(x), ParsedAtom::Small(m)) => ParsedAtom::Small(dis(*x, *m)),
            _ => ParsedAtom::Big(dis_big(&pyn.to_biguint(), &mask_hi.to_biguint())),
        };

        let feined_lo = fein(lo);
        con_atoms(hi, feined_lo)
    } else {
        pyn
    }
}

fn tail(m: u64) -> u64 {
    feen(4, 0xffff, 0x1_0000, 0xffff * 0x1_0000, m)
}

fn fynd_big(cry: &BigUint) -> BigUint {
    let one_16 = BigUint::from(0x1_0000u32);
    let max_32 = BigUint::from(0xffff_ffffu32);
    let one_32 = BigUint::from(0x1_0000_0000u64);
    let max_64 = BigUint::from(u64::MAX);

    if cry >= &one_16 && cry <= &max_32 {
        let x = cry.to_u64().unwrap();
        return BigUint::from(fynd_u64(x));
    }

    if cry >= &one_32 && cry <= &max_64 {
        let lo = cry & &max_32;
        let hi = cry - &lo;
        let lo_f = BigUint::from(fynd_u64(lo.to_u64().unwrap()));
        return hi + lo_f;
    }

    cry.clone()
}

pub fn fynd_u64(cry: u64) -> u64 {
    if cry >= 0x1_0000 && cry <= 0xffff_ffff {
        return 0x1_0000 + tail(cry - 0x1_0000);
    }

    if cry >= 0x1_0000_0000 && cry <= 0xffff_ffff_ffff_ffff {
        let lo = dis(cry, 0xffff_ffff);
        let hi = dis(cry, 0xffff_ffff_0000_0000);
        return con(hi, fynd_u64(lo));
    }

    cry
}

pub fn twid<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        just('0').ignore_then(base32()).try_map(|s, span| {
            let atom = base32_to_atom(s);
            cue_simple(atom)
                .map(Coin::Blob)
                .map_err(|e| Rich::custom(span, format!("Failed to +cue: {}", e)))
        }),
        crub(),
    ))
}

pub fn cue_simple(buffer: ParsedAtom) -> Result<NounExpr, Box<dyn std::error::Error>> {
    let bits = atom_to_bits(&buffer);
    let mut backrefs = HashMap::new();
    let (noun, _) = cue_inner(&bits, 0, &mut backrefs)?;
    Ok(noun)
}

fn noun_hash(noun: &NounExpr) -> u64 {
    let mut hasher = DefaultHasher::new();
    noun.hash(&mut hasher);
    hasher.finish()
}

pub fn jam_simple(noun: NounExpr) -> ParsedAtom {
    let mut bits = Vec::new();
    let mut backrefs = HashMap::new();
    let mut stack = vec![noun];

    while let Some(current) = stack.pop() {
        if let Some(&offset) = backrefs.get(&current) {
            let use_backref = match &current {
                NounExpr::ParsedAtom(atom) => {
                    let atom_bits = mat_bits(atom).len();
                    let offset_bits = mat_bits(&offset_to_atom(offset)).len();
                    offset_bits < atom_bits
                }
                NounExpr::Cell(_, _) => true,
            };

            if use_backref {
                bits.push(true);
                bits.push(true);
                bits.extend(mat_bits(&offset_to_atom(offset)));
                continue;
            }
        }

        let offset = bits.len();
        backrefs.insert(current.clone(), offset);

        match current {
            NounExpr::ParsedAtom(atom) => {
                bits.push(false);
                bits.extend(mat_bits(&atom));
            }
            NounExpr::Cell(head, tail) => {
                bits.push(true);
                bits.push(false);
                stack.push(*tail);
                stack.push(*head);
            }
        }
    }

    bits_to_atom(&bits)
}

fn offset_to_atom(offset: usize) -> ParsedAtom {
    if offset <= u128::MAX as usize {
        ParsedAtom::Small(offset as u128)
    } else {
        ParsedAtom::Big(BigUint::from(offset))
    }
}

fn mat_bits(atom: &ParsedAtom) -> Vec<bool> {
    let n = atom_bit_len(atom); // = met0(atom): number of bits needed to represent the atom

    let mut bits = Vec::new();

    if n == 0 {
        bits.push(true);
        return bits;
    }

    let k = usize_bit_len(n); // met0(n)

    bits.extend(std::iter::repeat(false).take(k));

    bits.push(true);

    if k > 1 {
        let offset = n - (1usize << (k - 1)); // same as n & ((1 << (k-1)) - 1)
        for i in 0..(k - 1) {
            bits.push((offset >> i) & 1 == 1);
        }
    }

    for i in 0..n {
        bits.push(atom_get_bit(atom, i as u64));
    }

    bits
}

fn usize_bit_len(x: usize) -> usize {
    if x == 0 {
        1
    } else {
        (usize::BITS - x.leading_zeros()) as usize
    }
}

fn atom_bit_len(atom: &ParsedAtom) -> usize {
    match atom {
        ParsedAtom::Small(0) => 0,
        ParsedAtom::Small(x) => (128 - x.leading_zeros() as usize),
        ParsedAtom::Big(x) => x.bits() as usize,
    }
}

fn atom_get_bit(atom: &ParsedAtom, i: u64) -> bool {
    match atom {
        ParsedAtom::Small(x) => i < 128 && ((x >> i) & 1 == 1),
        ParsedAtom::Big(x) => {
            let byte_index = (i / 8) as usize;
            let bit_index = (i % 8) as u8;
            let bytes = x.to_bytes_le();
            if byte_index < bytes.len() {
                let byte = bytes[byte_index];
                (byte >> bit_index) & 1 == 1
            } else {
                false
            }
        }
    }
}

fn bits_to_atom(bits: &[bool]) -> ParsedAtom {
    if bits.is_empty() {
        return ParsedAtom::Small(0);
    }

    let len = bits.len();

    if len <= 128 {
        let mut val: u128 = 0;
        for (i, &bit) in bits.iter().enumerate() {
            if bit {
                val |= 1u128 << i;
            }
        }
        ParsedAtom::Small(val)
    } else {
        let mut big = BigUint::from(0u32);
        for (i, &bit) in bits.iter().enumerate() {
            if bit {
                big += BigUint::from(1u32) << i;
            }
        }
        ParsedAtom::Big(big)
    }
}

#[derive(Debug)]
enum ParseAction {
    Start(u64),                       // start parsing noun at cursor
    CellHeadDone(u64, Box<NounExpr>), // head done, now parse tail at given cursor
    FinishCell(Box<NounExpr>, Box<NounExpr>),
    StoreBackref(u64),
}
fn rub_backref(bits: &[bool], cursor: &mut usize) -> Result<u64, Box<dyn std::error::Error>> {
    let size = get_size(bits, cursor)?;
    if size == 0 {
        return Ok(0);
    }
    if size > 64 {
        return Err("backref offset too large (>64 bits)".into());
    }
    if *cursor + size as usize > bits.len() {
        return Err("not enough bits for backref".into());
    }

    let mut val: u64 = 0;
    for i in 0..size {
        if bits[*cursor + i as usize] {
            val |= 1u64 << i;
        }
    }
    *cursor += size as usize;
    Ok(val)
}

fn rub_atom(bits: &[bool], cursor: &mut usize) -> Result<ParsedAtom, Box<dyn std::error::Error>> {
    let size = get_size(bits, cursor)?;

    if size == 0 {
        return Ok(ParsedAtom::Small(0));
    }

    if *cursor + size as usize > bits.len() {
        return Err("not enough bits for rub atom payload".into());
    }

    // Read `size` bits, LSB-first → value = sum bit_i * 2^i
    if size <= 128 {
        let mut val: u128 = 0;
        for i in 0..size {
            if bits[*cursor + i as usize] {
                val |= 1u128 << i;
            }
        }
        *cursor += size as usize;
        Ok(ParsedAtom::Small(val))
    } else {
        // Use BigUint
        let mut big = BigUint::from(0u32);
        for i in 0..size {
            if bits[*cursor + i as usize] {
                big += BigUint::from(1u32) << i;
            }
        }
        *cursor += size as usize;
        Ok(ParsedAtom::Big(big))
    }
}

fn get_size(bits: &[bool], cursor: &mut usize) -> Result<u64, &'static str> {
    let start = *cursor;
    // Count leading zeros
    while *cursor < bits.len() && !bits[*cursor] {
        *cursor += 1;
    }

    if *cursor >= bits.len() {
        return Err("unexpected EOF in rub size prefix");
    }

    let c = (*cursor - start) as u32; // number of leading zeros
    *cursor += 1; // consume the '1'

    if c == 0 {
        Ok(0)
    } else {
        // Read c-1 bits
        if *cursor + (c - 1) as usize > bits.len() {
            return Err("not enough bits for rub size field");
        }

        let mut x = 0u64;
        for i in 0..(c - 1) {
            if bits[*cursor + i as usize] {
                x |= 1u64 << i; // LSB-first: first bit = 2^0
            }
        }
        *cursor += (c - 1) as usize;

        let size = (1u64 << (c - 1)) + x;
        Ok(size)
    }
}

fn atom_to_bits(atom: &ParsedAtom) -> Vec<bool> {
    match atom {
        ParsedAtom::Small(x) => {
            let mut bits = Vec::with_capacity(128);
            for i in 0..128 {
                bits.push((x >> i) & 1 == 1);
            }
            // Trim trailing zeros beyond highest set bit? Not needed — cue stops when done.
            bits
        }
        ParsedAtom::Big(x) => {
            // Convert to little-endian bytes, then bits
            let bytes = x.to_bytes_le();
            let mut bits = Vec::new();
            for &byte in &bytes {
                for i in 0..8 {
                    bits.push((byte >> i) & 1 == 1);
                }
            }
            // Pad to next multiple of 8? Not necessary.
            bits
        }
    }
}

fn cue_inner(
    // rename
    bits: &[bool],
    cursor: usize,
    backrefs: &mut HashMap<u64, NounExpr>,
) -> Result<(NounExpr, usize), Box<dyn std::error::Error>> {
    if cursor >= bits.len() {
        return Err("unexpected EOF".into());
    }

    let tag0 = bits[cursor];
    if !tag0 {
        let mut cur = cursor + 1;
        let atom = rub_atom(bits, &mut cur)?;
        let noun = NounExpr::ParsedAtom(atom);
        backrefs.insert(cursor as u64, noun.clone());
        Ok((noun, cur))
    } else {
        if cursor + 1 >= bits.len() {
            return Err("unexpected EOF after tag 1".into());
        }
        let tag1 = bits[cursor + 1];
        if !tag1 {
            let mut cur = cursor + 2;
            let (head, next) = cue_inner(bits, cur, backrefs)?;
            cur = next;
            let (tail, next2) = cue_inner(bits, cur, backrefs)?;
            cur = next2;
            let noun = NounExpr::Cell(Box::new(head), Box::new(tail));
            backrefs.insert(cursor as u64, noun.clone());
            Ok((noun, cur))
        } else {
            let mut cur = cursor + 2;
            let offset = rub_backref(bits, &mut cur)?;

            let noun = backrefs
                .get(&(offset))
                .cloned()
                .ok_or_else(|| format!("backref to {} not found", offset))?;
            Ok((noun, cur))
        }
    }
}

pub fn crub<'src>() -> impl Parser<'src, &'src str, Coin, Err<'src>> {
    choice((
        absolute_date().map(|d| Coin::Dime("da".to_string(), d)),
        relative_date().map(|d| Coin::Dime("dr".to_string(), d)),
        phonemic_name().map(|p| Coin::Dime("p".to_string(), p)),
        just('.')
            .ignore_then(urs())
            .map(|atom| Coin::Dime("ta".to_string(), atom)),
        just('~')
            .ignore_then(urx())
            .map(|atom| Coin::Dime("t".to_string(), atom)),
        just('-')
            .ignore_then(urx())
            .map(|atom| Coin::Dime("c".to_string(), taft(&atom))),
    ))
}

//  +rump: name/hoon or name+hoon
//
pub fn constant_separator_hoon<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('$').to(Hoon::Rock(
            "tas".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(0)),
        )),
        symbol().map(|s| Hoon::Rock("tas".to_string(), NounExpr::ParsedAtom(string_to_atom(s)))),
        number().map(|(p, q)| Hoon::Rock(p, NounExpr::ParsedAtom(q))),
        just('&').to(Hoon::Rock(
            "f".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(0)),
        )),
        just('|').to(Hoon::Rock(
            "f".to_string(),
            NounExpr::ParsedAtom(ParsedAtom::Small(1)),
        )),
        just('~').to(Hoon::Bust(BaseType::Null)),
    ))
    .then(just('+').or(just('/')).ignore_then(hoon.clone()))
    .map(|(p, hoon)| Hoon::Pair(Box::new(p), Box::new(hoon)))
}

//  `@p`q
//
pub fn tic_aura<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    aura_text()
        .then_ignore(just("`"))
        .then(hoon_wide.clone())
        .map(|(a, b)| {
            Hoon::KetLus(
                Box::new(Hoon::Sand(a, NounExpr::ParsedAtom(ParsedAtom::Small(0)))),
                Box::new(Hoon::KetLus(
                    Box::new(Hoon::Sand(
                        "$".to_string(),
                        NounExpr::ParsedAtom(ParsedAtom::Small(0)),
                    )),
                    Box::new(b),
                )),
            )
        })
}

pub fn tic_cell_construction<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide.clone().map(|h| {
        Hoon::Pair(
            Box::new(Hoon::Rock(
                "n".to_string(),
                NounExpr::ParsedAtom(ParsedAtom::Small(0)),
            )),
            Box::new(h),
        )
    })
}

pub fn parenthesis_spec<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    hoon_wide
        .clone()
        .then(
            just(' ')
                .ignore_then(spec_wide.clone())
                .repeated()
                .collect::<Vec<_>>()
                .or_not()
                .map(|specs| specs.unwrap_or_default()),
        )
        .delimited_by(just('('), just(')'))
        .map(|(name, specs)| Spec::Make(name, specs))
}

pub fn reference_spec<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    let lower = any().filter(|c: &char| matches!(c, 'a'..='z'));

    let ident_tail = any().filter(|c: &char| c.is_ascii_alphanumeric());

    let ident = lower
        .then(ident_tail.repeated().collect::<Vec<char>>())
        .to(());

    let special = any().filter(|c: &char| matches!(c, '$' | '^' | ',')).to(());

    let guard = ident.or(special).rewind();

    // prevents this parser from matching
    //  inputs that starts with: "([a-z][a-zA-Z0-9]*)|[\$\^\,]"
    guard.rewind().ignore_then(
        winglist()
            .separated_by(just(':'))
            .at_least(1)
            .collect::<Vec<_>>()
            .map(|wings: Vec<WingType>| {
                let (first, rest) = wings.split_first().unwrap();
                Spec::Like(first.clone(), rest.to_vec())
            }),
    )
}

pub fn two_hoons_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, (Hoon, Hoon), Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
}

pub fn two_hoons_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, (Hoon, Hoon), Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
}

pub fn three_hoons_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, ((Hoon, Hoon), Hoon), Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
        .then_ignore(gap())
        .then(hoon.clone())
}

pub fn three_hoons_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, ((Hoon, Hoon), Hoon), Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
}

pub fn two_specs_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Spec), Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(spec.clone())
}

pub fn two_specs_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Spec), Err<'src>> {
    two_specs_tall(spec.clone())
        .then_ignore(gap())
        .then_ignore(just("=="))
}

pub fn two_specs_closed_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Spec), Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
}

pub fn hoon_spec_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Hoon, Spec), Err<'src>> {
    hoon_wide
        .clone()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
}

pub fn hoon_spec_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Hoon, Spec), Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .then(spec.clone())
}

pub fn spec_hoon_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Hoon), Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .then(hoon.clone())
}

pub fn spec_hoon_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (Spec, Hoon), Err<'src>> {
    spec_wide
        .clone()
        .then_ignore(just(' '))
        .then(hoon_wide.clone())
}

pub fn name_spec_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (String, Spec), Err<'src>> {
    gap()
        .ignore_then(symbol())
        .then_ignore(gap())
        .then(spec.clone())
}

pub fn name_spec_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, (String, Spec), Err<'src>> {
    gap()
        .ignore_then(symbol())
        .then_ignore(gap())
        .then(spec.clone())
        .then_ignore(just("=="))
}

pub fn name_spec_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec> + Clone,
) -> impl Parser<'src, &'src str, (String, Spec), Err<'src>> {
    symbol()
        .then_ignore(just(' '))
        .then(spec_wide.clone())
        .delimited_by(just('('), just(')'))
}

pub fn one_hoon_closed_wide<'src>(
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    hoon_wide.clone().delimited_by(just('('), just(')'))
}

pub fn one_hoon_closed_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .then_ignore(gap())
        .delimited_by(just('='), just('='))
}

pub fn one_spec_closed_wide<'src>(
    spec_wide: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    spec_wide.clone().delimited_by(just('('), just(')'))
}

pub fn one_spec_closed_tall<'src>(
    spec: impl ParserExt<'src, Spec>,
) -> impl Parser<'src, &'src str, Spec, Err<'src>> {
    gap()
        .ignore_then(spec.clone())
        .then_ignore(gap())
        .delimited_by(just('='), just('='))
}

pub fn wrap_hoon_with_trace(
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl for<'src> Fn(Hoon, &mut MapExtra<'src, '_, &'src str, Err<'src>>) -> Hoon + Clone {
    move |node, e| {
        let spot = chumsky_spot_to_hoon_spot((e.span().start(), e.span().end()), &wer, &linemap);

        match node {
            Hoon::Dbug(existing_spot, inner) => {
                if existing_spot == spot {
                    Hoon::Dbug(existing_spot, inner)
                } else {
                    Hoon::Dbug(spot, Box::new(Hoon::Dbug(existing_spot, inner)))
                }
            }
            other => Hoon::Dbug(spot, Box::new(other)),
        }
    }
}

pub fn wrap_spec_with_trace(
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl for<'src> Fn(Spec, &mut MapExtra<'src, '_, &'src str, Err<'src>>) -> Spec + Clone {
    move |node, e| {
        let spot = chumsky_spot_to_hoon_spot((e.span().start(), e.span().end()), &wer, &linemap);

        match node {
            Spec::Dbug(existing_spot, inner) => {
                if existing_spot == spot {
                    Spec::Dbug(existing_spot, inner)
                } else {
                    Spec::Dbug(spot, Box::new(Spec::Dbug(existing_spot, inner)))
                }
            }
            other => Spec::Dbug(spot, Box::new(other)),
        }
    }
}

fn chumsky_spot_to_hoon_spot(span: (usize, usize), wer: &Path, linemap: &Arc<LineMap>) -> Spot {
    let (start, end) = span;

    let (sl, sc) = linemap.line_col(start);
    let (el, ec) = linemap.line_col(end);

    Spot {
        p: wer.clone(),
        q: Pint {
            p: (sl as u64, sc as u64),
            q: (el as u64, ec as u64),
        },
    }
}

pub fn print_noun(noun: &Noun, max_depth: usize, current_depth: usize) -> String {
    if current_depth >= max_depth {
        return "...".to_string();
    }

    match noun.as_either_atom_cell() {
        Left(atom) => format!("{:?}", atom),

        Right(cell) => {
            let head = cell.head();
            let tail = cell.tail();

            let head_is_atom = head.as_either_atom_cell().is_left();
            let tail_is_atom = tail.as_either_atom_cell().is_left();

            if head_is_atom && tail_is_atom {
                format!(
                    "[{} {}]",
                    print_noun(&head, max_depth, current_depth + 1),
                    print_noun(&tail, max_depth, current_depth + 1),
                )
            } else {
                let indent = "  ".repeat(current_depth);
                let inner_indent = "  ".repeat(current_depth + 1);

                format!(
                    "[\n{}{}\n{}{}\n{}]",
                    inner_indent,
                    print_noun(&head, max_depth, current_depth + 1),
                    inner_indent,
                    print_noun(&tail, max_depth, current_depth + 1),
                    indent,
                )
            }
        }
    }
}

// pub fn print_noun(
//     noun: &Noun,
//     max_depth: usize,
//     current_depth: usize,
// ) -> String {
//     if current_depth >= max_depth {
//         return "...".to_string();
//     }

//     let indent = "  ".repeat(current_depth);

//     match noun.as_either_atom_cell() {
//         Left(atom) => format!("{:?}", atom),
//         Right(cell) => format!(
//             "[\n{}  {}\n{}  {}\n{}]",
//             indent,
//             print_noun(&cell.head(), max_depth, current_depth + 1),
//             indent,
//             print_noun(&cell.tail(), max_depth, current_depth + 1),
//             indent,
//         ),
//     }
// }

fn skip_dbug(mut n: Noun) -> Noun {
    loop {
        let cell = match n.cell() {
            Some(c) => c,
            None => return n,
        };

        let head = match cell.head().as_atom() {
            Ok(a) => a,
            Err(_) => return n,
        };

        if unsafe { !head.as_noun().raw_equals(&D(tas!(b"dbug"))) } {
            return n;
        }

        let tail_cell = match cell.tail().as_cell() {
            Ok(c) => c,
            Err(_) => return n,
        };

        n = tail_cell.tail();
    }
}

pub fn diff_noun(a: &Noun, b: &Noun, printed: &mut bool) -> Result<(), ()> {
    let a = skip_dbug(*a);
    let b = skip_dbug(*b);

    if slab_noun_equality(&a, &b) {
        return Ok(());
    }

    match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
        (Right(ac), Right(bc)) => {
            if diff_noun(&ac.head(), &bc.head(), printed).is_err() {
                if !*printed {
                    print_context(&a, &b);
                    *printed = true;
                }
                return Err(());
            }

            if diff_noun(&ac.tail(), &bc.tail(), printed).is_err() {
                if !*printed {
                    print_context(&a, &b);
                    *printed = true;
                }
                return Err(());
            }

            Ok(())
        }

        _ => Err(()),
    }
}

fn print_context(a: &Noun, b: &Noun) {
    println!("Mismatch in subtree:");
    println!("expected: {}", print_noun(a, 10, 0));
    println!("actual:   {}", print_noun(b, 10, 0));
}

pub fn diff_and_report(a: &Noun, b: &Noun) {
    let mut printed = false;
    if diff_noun(a, b, &mut printed).is_ok() {
        println!("Test passed!");
    }
}

fn atom_to_tas_string(atom: &DirectAtom) -> String {
    let val: u128 = atom.data() as u128;
    if val == 0 {
        return String::new();
    }

    let bytes = val.to_le_bytes();
    let mut null_seen = false;
    let mut valid = true;
    let mut len = 0;

    for &b in &bytes {
        if b == 0 {
            null_seen = true;
        } else if null_seen {
            valid = false;
            break;
        } else if !b.is_ascii_lowercase() && b != b'-' {
            valid = false;
            break;
        } else {
            len += 1;
        }

        // Cap at 126 bytes (Urbit tas limit)
        if len > 126 {
            valid = false;
            break;
        }
    }

    if valid && len > 0 {
        format!("%{}", unsafe {
            std::str::from_utf8_unchecked(&bytes[..len])
        })
    } else {
        String::new()
    }
}

pub fn hoon_to_noun(slab: &mut NounSlab, hoon: &Hoon) -> Noun {
    use Hoon::*;

    match hoon {
        Pair(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[p, q])
        }
        ZapZap => T(slab, &[D(tas!(b"zpzp")), D(0)]),
        Axis(a) => T(slab, &[D(0), D(*a)]),
        Base(bt) => {
            let bt_noun = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"base")), bt_noun])
        }
        Bust(bt) => {
            let bt_noun = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"bust")), bt_noun])
        }
        Dbug(spot, h) => {
            let spot_noun = spot_to_noun(slab, spot);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"dbug")), spot_noun, h_noun])
        }
        Eror(msg) => {
            let msg_noun = cord_to_noun(slab, msg);
            T(slab, &[D(tas!(b"eror")), msg_noun])
        }
        Hand(typ, nock) => {
            let typ_noun = type_to_noun(slab, typ);
            let nock_noun = nock_to_noun(slab, nock);
            T(slab, &[D(tas!(b"hand")), typ_noun, nock_noun])
        }
        Note(note, h) => {
            let note_noun = note_to_noun(slab, note);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"note")), note_noun, h_noun])
        }
        Fits(h, wing) => {
            let h_noun = hoon_to_noun(slab, h);
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"fits")), h_noun, wing_noun])
        }
        Knit(woofs) => {
            let woofs_noun: Vec<_> = woofs.iter().map(|w| woof_to_noun(slab, w)).collect();
            let list = list_to_noun(slab, woofs_noun);
            T(slab, &[D(tas!(b"knit")), list])
        }
        Leaf(tag, atom) => {
            let tag_noun = term_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            T(slab, &[D(tas!(b"leaf")), tag_noun, atom_noun])
        }
        Limb(name) => {
            let name_noun = term_to_noun(slab, name);
            T(slab, &[D(tas!(b"limb")), name_noun])
        }
        Lost(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"lost")), h_noun])
        }
        Rock(au, expr) => {
            let au_noun = term_to_noun(slab, au);
            let expr_noun = noun_expr_to_noun(slab, expr);
            T(slab, &[D(tas!(b"rock")), au_noun, expr_noun])
        }
        Sand(au, expr) => {
            let au_noun = term_to_noun(slab, au);
            let expr_noun = noun_expr_to_noun(slab, expr);
            T(slab, &[D(tas!(b"sand")), au_noun, expr_noun])
        }
        Tell(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"tell")), list])
        }
        Tune(tune) => {
            let tune_noun = term_or_tune_to_noun(slab, tune);
            T(slab, &[D(tas!(b"tune")), tune_noun])
        }
        Wing(wing) => {
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"wing")), wing_noun])
        }
        Yell(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"yell")), list])
        }
        Xray(manx) => {
            let manx_noun = manx_to_noun(slab, manx);
            T(slab, &[D(tas!(b"xray")), manx_noun])
        }
        BarBuc(tagnames, spec) => {
            let tags_noun: Vec<_> = tagnames.iter().map(|s| term_to_noun(slab, s)).collect();
            let list = list_to_noun(slab, tags_noun);
            let spec_noun = spec_to_noun(slab, spec);
            T(slab, &[D(tas!(b"brbc")), list, spec_noun])
        }
        BarCab(spec, alas, tomes) => {
            let spec_noun = spec_to_noun(slab, spec);
            let alas_noun = alas_to_noun(slab, alas);

            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brcb")), spec_noun, alas_noun, tomes_noun])
        }
        BarCol(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"brcl")), p, q])
        }
        BarCen(prefix, tomes) => {
            let prefix_noun = prefix
                .as_ref()
                .map_or_else(|| D(0u64), |s| term_to_noun(slab, s));
            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brcn")), prefix_noun, tomes_noun])
        }
        BarDot(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brdt")), p])
        }
        BarKet(p, tomes) => {
            let p_noun = hoon_to_noun(slab, p);
            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brkt")), p_noun, tomes_noun])
        }
        BarHep(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brhp")), p])
        }
        BarSig(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brsg")), spec_noun, p_noun])
        }
        BarTar(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brtr")), spec_noun, p_noun])
        }
        BarTis(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brts")), spec_noun, p_noun])
        }
        BarPat(prefix, tomes) => {
            let prefix_noun = prefix
                .as_ref()
                .map_or_else(|| D(0u64), |s| term_to_noun(slab, s));
            let mut tomes_pairs = Vec::new();
            for (k, tome) in tomes {
                let k_noun = term_to_noun(slab, k);
                let tome_noun = tome_to_noun(slab, tome);
                tomes_pairs.push((k_noun, tome_noun));
            }
            let tomes_noun = map_to_noun(slab, tomes_pairs);
            T(slab, &[D(tas!(b"brpt")), prefix_noun, tomes_noun])
        }
        BarWut(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"brwt")), p])
        }
        ColCab(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"clcb")), p, q])
        }
        ColKet(a, b, c, d) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            let d = hoon_to_noun(slab, d);
            T(slab, &[D(tas!(b"clkt")), a, b, c, d])
        }
        ColHep(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"clhp")), p, q])
        }
        ColLus(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"clls")), a, b, c])
        }
        ColSig(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"clsg")), list])
        }
        ColTar(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"cltr")), list])
        }
        CenCab(wing, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"cncb")), wing_noun, list])
        }
        CenDot(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"cndt")), p, q])
        }
        CenHep(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"cnhp")), p, q])
        }
        CenCol(p, hoons) => {
            let p = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"cncl")), p, list])
        }
        CenTar(wing, p, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let p_noun = hoon_to_noun(slab, p);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"cntr")), wing_noun, p_noun, list])
        }
        CenKet(a, b, c, d) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            let d = hoon_to_noun(slab, d);
            T(slab, &[D(tas!(b"cnkt")), a, b, c, d])
        }
        CenLus(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"cnls")), a, b, c])
        }
        CenSig(wing, p, hoons) => {
            let wing_noun = wing_to_noun(slab, wing);
            let p_noun = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"cnsg")), wing_noun, p_noun, list])
        }
        CenTis(wing, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"cnts")), wing_noun, list])
        }
        DotKet(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"dtkt")), spec_noun, p_noun])
        }
        DotLus(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"dtls")), p])
        }
        DotTar(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"dttr")), p, q])
        }
        DotTis(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"dtts")), p, q])
        }
        DotWut(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"dtwt")), p])
        }
        KetBar(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktbr")), p])
        }
        KetDot(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"ktdt")), p, q])
        }
        KetLus(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"ktls")), p, q])
        }
        KetHep(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"kthp")), spec_noun, p_noun])
        }
        KetPam(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktpm")), p])
        }
        KetSig(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktsg")), p])
        }
        KetTis(skin, p) => {
            let skin_noun = skin_to_noun(slab, skin);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktts")), skin_noun, p_noun])
        }
        KetWut(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"ktwt")), p])
        }
        KetTar(spec) => {
            let spec_noun = spec_to_noun(slab, spec);
            T(slab, &[D(tas!(b"kttr")), spec_noun])
        }
        KetCol(spec) => {
            let spec_noun = spec_to_noun(slab, spec);
            T(slab, &[D(tas!(b"ktcl")), spec_noun])
        }
        SigBar(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgbr")), p, q])
        }
        SigCab(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgcb")), p, q])
        }
        SigCen(chum, p, tyre, q) => {
            let chum_noun = chum_to_noun(slab, chum);
            let p_noun = hoon_to_noun(slab, p);
            let tyre_noun = tyre_to_noun(slab, tyre);
            let q_noun = hoon_to_noun(slab, q);
            T(
                slab,
                &[D(tas!(b"sgcn")), chum_noun, p_noun, tyre_noun, q_noun],
            )
        }
        SigFas(chum, p) => {
            let chum_noun = chum_to_noun(slab, chum);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sgfs")), chum_noun, p_noun])
        }
        SigGal(term_or_pair, p) => {
            let term_noun = term_or_pair_to_noun(slab, term_or_pair);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sggl")), term_noun, p_noun])
        }
        SigGar(term_or_pair, p) => {
            let term_noun = term_or_pair_to_noun(slab, term_or_pair);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sggr")), term_noun, p_noun])
        }
        SigBuc(tag, p) => {
            let tag_noun = term_to_noun(slab, tag);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sbbc")), tag_noun, p_noun])
        }
        SigLus(n, p) => {
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"sgls")), D(*n), p_noun])
        }
        SigPam(n, p, q) => {
            let p_noun = hoon_to_noun(slab, p);
            let q_noun = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgpm")), D(*n), p_noun, q_noun])
        }
        SigTis(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"spts")), p, q])
        }
        SigWut(n, a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"sgwt")), D(*n), a, b, c])
        }
        SigZap(p, q) => {
            let p = hoon_to_noun(slab, p);
            let q = hoon_to_noun(slab, q);
            T(slab, &[D(tas!(b"sgzp")), p, q])
        }
        MicTis(marl) => {
            let marl_noun = marl_to_noun(slab, marl);
            T(slab, &[D(tas!(b"mcts")), marl_noun])
        }
        MicCol(p, hoons) => {
            let p = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"mccl")), p, list])
        }
        MicFas(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"mcfs")), p])
        }
        MicGal(spec, a, b, c) => {
            let spec_noun = spec_to_noun(slab, spec);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"mcgl")), spec_noun, a, b, c])
        }
        MicSig(p, hoons) => {
            let p = hoon_to_noun(slab, p);
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"mcsg")), p, list])
        }
        MicMic(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"mcmc")), spec_noun, p_noun])
        }
        TisBar(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"tsbr")), spec_noun, p_noun])
        }
        TisCol(pairs, p) => {
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(w, h)| {
                    let w_noun = wing_to_noun(slab, w);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[w_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"tscl")), list, p_noun])
        }
        TisFas(skin, a, b) => {
            let skin_noun = skin_to_noun(slab, skin);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsfs")), skin_noun, a, b])
        }
        TisMic(skin, a, b) => {
            let skin_noun = skin_to_noun(slab, skin);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsmc")), skin_noun, a, b])
        }
        TisDot(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsdt")), wing_noun, a, b])
        }
        TisWut(wing, a, b, c) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"tswt")), wing_noun, a, b, c])
        }
        TisGal(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsgl")), a, b])
        }
        TisHep(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tshp")), a, b])
        }
        TisGar(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsgr")), a, b])
        }
        TisKet(skin, wing, a, b) => {
            let skin_noun = skin_to_noun(slab, skin);
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tskt")), skin_noun, wing_noun, a, b])
        }
        TisLus(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tsls")), a, b])
        }
        TisSig(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"tssg")), list])
        }
        TisTar((name, spec_opt), a, b) => {
            let name_noun = term_to_noun(slab, name);
            let spec_noun = spec_opt
                .as_ref()
                .map_or_else(|| D(0u64), |s| spec_to_noun(slab, s));
            let name_spec = T(slab, &[name_noun, spec_noun]);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tstr")), name_spec, a, b])
        }
        TisCom(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"tscm")), a, b])
        }
        WutBar(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"wtbr")), list])
        }
        WutHep(wing, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(spec, h)| {
                    let spec_noun = spec_to_noun(slab, spec);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[spec_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"wthp")), wing_noun, list])
        }
        WutCol(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"wtcl")), a, b, c])
        }
        WutDot(a, b, c) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            let c = hoon_to_noun(slab, c);
            T(slab, &[D(tas!(b"wtdt")), a, b, c])
        }
        WutKet(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtkt")), wing_noun, a, b])
        }
        WutGal(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtgl")), a, b])
        }
        WutGar(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtgr")), a, b])
        }
        WutLus(wing, a, pairs) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let pairs_noun: Vec<_> = pairs
                .iter()
                .map(|(spec, h)| {
                    let spec_noun = spec_to_noun(slab, spec);
                    let h_noun = hoon_to_noun(slab, h);
                    T(slab, &[spec_noun, h_noun])
                })
                .collect();
            let list = list_to_noun(slab, pairs_noun);
            T(slab, &[D(tas!(b"wtls")), wing_noun, a, list])
        }
        WutPam(hoons) => {
            let hoons_noun: Vec<_> = hoons.iter().map(|h| hoon_to_noun(slab, h)).collect();
            let list = list_to_noun(slab, hoons_noun);
            T(slab, &[D(tas!(b"wtpm")), list])
        }
        WutPat(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtpt")), wing_noun, a, b])
        }
        WutSig(wing, a, b) => {
            let wing_noun = wing_to_noun(slab, wing);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"wtsg")), wing_noun, a, b])
        }
        WutHax(skin, wing) => {
            let skin_noun = skin_to_noun(slab, skin);
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"wthx")), skin_noun, wing_noun])
        }
        WutTis(spec, wing) => {
            let spec_noun = spec_to_noun(slab, spec);
            let wing_noun = wing_to_noun(slab, wing);
            T(slab, &[D(tas!(b"wtts")), spec_noun, wing_noun])
        }
        WutZap(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"wtzp")), p])
        }
        ZapCom(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"zpcm")), a, b])
        }
        ZapGar(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpgr")), p])
        }
        ZapGal(spec, p) => {
            let spec_noun = spec_to_noun(slab, spec);
            let p_noun = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpgl")), spec_noun, p_noun])
        }
        ZapMic(a, b) => {
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"zpmc")), a, b])
        }
        ZapTis(p) => {
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpts")), p])
        }
        ZapPat(wings, a, b) => {
            let wing_nouns: Vec<_> = wings.iter().map(|w| wing_to_noun(slab, w)).collect();
            let wings_noun = list_to_noun(slab, wing_nouns);
            let a = hoon_to_noun(slab, a);
            let b = hoon_to_noun(slab, b);
            T(slab, &[D(tas!(b"zppt")), wings_noun, a, b])
        }
        ZapWut(arg, p) => {
            let arg_noun = zpwt_arg_to_noun(slab, arg);
            let p = hoon_to_noun(slab, p);
            T(slab, &[D(tas!(b"zpwt")), arg_noun, p])
        }
    }
}

fn list_to_noun(slab: &mut NounSlab, nouns: Vec<Noun>) -> Noun {
    nouns
        .into_iter()
        .rev()
        .fold(D(0u64), |tail, head| T(slab, &[head, tail]))
}

fn map_to_noun(slab: &mut NounSlab, pairs: Vec<(Noun, Noun)>) -> Noun {
    let mut map = D(0);

    for (key, val) in pairs {
        map = put_into_map(slab, map, key, val);
    }

    map
}

fn put_into_map(slab: &mut NounSlab, map: Noun, key: Noun, val: Noun) -> Noun {
    if map.is_atom() && slab_noun_equality(&map, &D(0)) {
        let node = T(slab, &[key, val]);
        return T(slab, &[node, D(0), D(0)]);
    }

    let cell = map.as_cell().expect("non-empty map must be a cell");
    let node = cell.head();
    let children = cell.tail();
    let children_cell = children.as_cell().expect("children must be a cell");
    let left = children_cell.head();
    let right = children_cell.tail();

    let node_cell = node.as_cell().expect("node must be [key value]");
    let node_key = node_cell.head();
    let node_val = node_cell.tail();

    if slab_noun_equality(&key, &node_key) {
        if slab_noun_equality(&val, &node_val) {
            return map;
        } else {
            let new_node = T(slab, &[key, val]);
            return T(slab, &[new_node, left, right]);
        }
    }

    if unsafe { gor_slab(key, node_key).raw_equals(&YES) } {
        let new_left = put_into_map(slab, left, key, val);

        if new_left.is_atom() {
            if !slab_noun_equality(&new_left, &D(0)) {
                panic!("put returned unexpected atom");
            }
        }

        let new_left_cell = new_left
            .as_cell()
            .expect("new_left must be cell after insert");
        let new_left_node = new_left_cell.head();
        let new_left_node_key = new_left_node.as_cell().expect("node must be [k v]").head();

        if unsafe { mor_slab(node_key, new_left_node_key).raw_equals(&YES) } {
            T(slab, &[node, new_left, right])
        } else {
            let new_left_children = new_left_cell.tail();
            let new_left_children_cell = new_left_children.as_cell().expect("children cell");
            let new_left_right = new_left_children_cell.tail();

            let new_right = T(slab, &[node, new_left_right, right]);
            T(
                slab,
                &[new_left_node, new_left_children_cell.head(), new_right],
            )
        }
    } else {
        let new_right = put_into_map(slab, right, key, val);

        if new_right.is_atom() {
            if !slab_noun_equality(&new_right, &D(0)) {
                panic!("unexpected atom in new_right");
            }
        }

        let new_right_cell = new_right.as_cell().expect("new_right must be cell");
        let new_right_node = new_right_cell.head();
        let new_right_node_key = new_right_node.as_cell().expect("node must be [k v]").head();

        if unsafe { mor_slab(node_key, new_right_node_key).raw_equals(&YES) } {
            T(slab, &[node, left, new_right])
        } else {
            let new_right_children = new_right_cell.tail();
            let new_right_children_cell = new_right_children.as_cell().expect("children cell");
            let new_right_left = new_right_children_cell.head();

            let new_left = T(slab, &[node, left, new_right_left]);
            T(
                slab,
                &[new_right_node, new_left, new_right_children_cell.tail()],
            )
        }
    }
}

fn term_to_noun(slab: &mut NounSlab, s: &str) -> Noun {
    let atom = term_to_atom(s.to_string());
    atom_to_noun(slab, &atom)
}

fn cord_to_noun(slab: &mut NounSlab, s: &str) -> Noun {
    let atom = string_to_atom(s.to_string());
    atom_to_noun(slab, &atom)
}

fn atom_to_noun(slab: &mut NounSlab, atom: &ParsedAtom) -> Noun {
    match atom {
        ParsedAtom::Small(n) => {
            if *n <= DIRECT_MAX as u128 {
                D(*n as u64)
            } else {
                let bytes = n.to_le_bytes();
                let trimmed_len = bytes.iter().rev().take_while(|&&b| b == 0).count();
                let trimmed = &bytes[..bytes.len() - trimmed_len];
                let bytes_slice = if trimmed.is_empty() { &[0u8] } else { trimmed };
                let bytes = Bytes::copy_from_slice(bytes_slice);
                Atom::from_bytes(slab, &bytes).as_noun()
            }
        }
        ParsedAtom::Big(b) => {
            let ubig: UBig = UBig::from_le_bytes(b.to_bytes_le().as_slice());
            Atom::from_ubig(slab, &ubig).as_noun()
        }
    }
}

fn biguint_to_ubig(b: &BigUint) -> UBig {
    UBig::from_le_bytes(&b.to_bytes_le())
}

fn opt_to_noun<T, F>(slab: &mut NounSlab, opt: &Option<T>, f: F) -> Noun
where
    F: FnOnce(&T) -> Noun,
{
    match opt {
        None => D(0u64),
        Some(x) => {
            let x_noun = f(x);
            T(slab, &[D(0u64), x_noun])
        }
    }
}

fn basetype_to_noun(slab: &mut NounSlab, bt: &BaseType) -> Noun {
    match bt {
        BaseType::NounExpr => D(tas!(b"noun")),
        BaseType::Cell => D(tas!(b"cell")),
        BaseType::Flag => D(tas!(b"flag")),
        BaseType::Null => D(tas!(b"null")),
        BaseType::Void => D(tas!(b"void")),
        BaseType::Atom(au) => {
            let at = term_to_noun(slab, au);
            T(slab, &[D(tas!(b"atom")), at])
        }
    }
}

fn noun_expr_to_noun(slab: &mut NounSlab, expr: &NounExpr) -> Noun {
    match expr {
        NounExpr::ParsedAtom(a) => atom_to_noun(slab, a),
        NounExpr::Cell(l, r) => {
            let l_noun = noun_expr_to_noun(slab, l);
            let r_noun = noun_expr_to_noun(slab, r);
            T(slab, &[l_noun, r_noun])
        }
    }
}

fn type_to_noun(slab: &mut NounSlab, typ: &Type) -> Noun {
    use Type::*;
    match typ {
        NounExpr => D(tas!(b"noun")),
        Void => D(tas!(b"void")),
        ParsedAtom(au, bits) => {
            let au_noun = term_to_noun(slab, au);
            let bits_noun = opt_to_noun(slab, bits, |n| D(*n));
            T(slab, &[D(tas!(b"atom")), au_noun, bits_noun])
        }
        Cell(l, r) => {
            let l = type_to_noun(slab, l);
            let r = type_to_noun(slab, r);
            T(slab, &[D(tas!(b"cell")), l, r])
        }
        Core(face, coil) => {
            let face_noun = type_to_noun(slab, face);
            let coil_noun = coil_to_noun(slab, coil);
            T(slab, &[D(tas!(b"core")), face_noun, coil_noun])
        }
        Face(face_type, inner) => {
            let face_noun = face_type_to_noun(slab, face_type);
            let inner_noun = type_to_noun(slab, inner);
            T(slab, &[D(tas!(b"face")), face_noun, inner_noun])
        }
        Fork(types) => {
            let types_vec: Vec<_> = types.iter().map(|t| type_to_noun(slab, t)).collect();
            let types_noun = list_to_noun(slab, types_vec);
            T(slab, &[D(tas!(b"fork")), types_noun])
        }
        Hint((inner, note), payload) => {
            let inner_noun = type_to_noun(slab, inner);
            let note_noun = note_to_noun(slab, note);
            let payload_noun = type_to_noun(slab, payload);
            let hint_inner = T(slab, &[inner_noun, note_noun]);
            T(slab, &[D(tas!(b"hint")), hint_inner, payload_noun])
        }
        Hold(typ, hoon) => {
            let typ_noun = type_to_noun(slab, typ);
            let hoon_noun = hoon_to_noun(slab, hoon);
            T(slab, &[D(tas!(b"hold")), typ_noun, hoon_noun])
        }
    }
}

fn face_type_to_noun(slab: &mut NounSlab, ft: &FaceType) -> Noun {
    match ft {
        FaceType::Term(s) => term_to_noun(slab, s),
        FaceType::Tune(tune) => {
            let tune_noun = tune_to_noun(slab, tune);
            T(slab, &[D(tas!(b"tune")), tune_noun])
        }
    }
}

fn coil_to_noun(slab: &mut NounSlab, coil: &Coil) -> Noun {
    let garb_noun = garb_to_noun(slab, &coil.p);
    let type_noun = type_to_noun(slab, &coil.q);
    let semi_noun = semi_noun_expr_to_noun(slab, &coil.r.0);

    let tomes_entries: Vec<_> = coil
        .r
        .1
        .iter()
        .map(|(k, v)| {
            let (what, v) = v;
            let k_noun = term_to_noun(slab, k);
            let inner_entries: Vec<_> = v
                .iter()
                .map(|(kk, vv)| (term_to_noun(slab, kk), hoon_to_noun(slab, vv)))
                .collect();
            let v_noun = map_to_noun(slab, inner_entries);
            (k_noun, T(slab, &[D(0), v_noun]))
        })
        .collect();

    let tomes_noun = map_to_noun(slab, tomes_entries);
    T(slab, &[garb_noun, type_noun, semi_noun, tomes_noun])
}

fn garb_to_noun(slab: &mut NounSlab, garb: &Garb) -> Noun {
    let name_noun = {
        if let Some(s) = &garb.name {
            term_to_noun(slab, s)
        } else {
            D(0)
        }
    };
    let poly_noun = poly_to_noun(slab, &garb.poly);
    let vair_noun = vair_to_noun(slab, &garb.vair);
    T(slab, &[name_noun, poly_noun, vair_noun])
}

fn poly_to_noun(_slab: &mut NounSlab, poly: &Poly) -> Noun {
    match poly {
        Poly::Wet => D(tas!(b"wet")),
        Poly::Dry => D(tas!(b"dry")),
    }
}

fn vair_to_noun(_slab: &mut NounSlab, vair: &Vair) -> Noun {
    match vair {
        Vair::Gold => D(tas!(b"gold")),
        Vair::Iron => D(tas!(b"iron")),
        Vair::Lead => D(tas!(b"lead")),
        Vair::Zinc => D(tas!(b"zinc")),
    }
}

fn semi_noun_expr_to_noun(slab: &mut NounSlab, (stencil, expr): &SemiNounExpr) -> Noun {
    let stencil_noun = stencil_to_noun(slab, stencil);
    let expr_noun = noun_expr_to_noun(slab, expr);
    T(slab, &[stencil_noun, expr_noun])
}

fn stencil_to_noun(slab: &mut NounSlab, st: &Stencil) -> Noun {
    match st {
        Stencil::Half { left, rite } => {
            let l = stencil_to_noun(slab, left);
            let r = stencil_to_noun(slab, rite);
            T(slab, &[D(tas!(b"half")), l, r])
        }
        Stencil::Full { blocks } => {
            let blocks_vec: Vec<_> = blocks.iter().map(|b| block_to_noun(slab, b)).collect();
            let blocks_noun = list_to_noun(slab, blocks_vec);
            T(slab, &[D(tas!(b"full")), blocks_noun])
        }
        Stencil::Lazy { fragment, resolve } => {
            let gate_noun = gate_to_noun(slab, resolve);
            T(slab, &[D(tas!(b"lazy")), D(*fragment), gate_noun])
        }
    }
}

fn block_to_noun(slab: &mut NounSlab, block: &Block) -> Noun {
    let paths: Vec<_> = block.iter().map(|path| path_to_noun(slab, path)).collect();
    list_to_noun(slab, paths)
}

fn path_to_noun(slab: &mut NounSlab, path: &Path) -> Noun {
    let knots: Vec<_> = path.iter().map(|k| cord_to_noun(slab, k)).collect();
    list_to_noun(slab, knots)
}

fn gate_to_noun(slab: &mut NounSlab, (spec, body): &Gate) -> Noun {
    let spec_noun = spec_to_noun(slab, spec);
    let body_noun = spec_to_noun(slab, body);
    T(slab, &[spec_noun, body_noun])
}

fn spec_to_noun(slab: &mut NounSlab, spec: &Spec) -> Noun {
    use Spec::*;
    match spec {
        Base(bt) => {
            let bt_noun = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"base")), bt_noun])
        }
        Dbug(spot, s) => {
            let spot_noun = spot_to_noun(slab, spot);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"dbug")), spot_noun, s_noun])
        }
        Leaf(tag, atom) => {
            let tag_noun = term_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            T(slab, &[D(tas!(b"leaf")), tag_noun, atom_noun])
        }
        Like(wing, wings) => {
            let wing_noun = wing_to_noun(slab, wing);
            let wings_vec: Vec<_> = wings.iter().map(|w| wing_to_noun(slab, w)).collect();
            let wings_noun = list_to_noun(slab, wings_vec);
            T(slab, &[D(tas!(b"like")), wing_noun, wings_noun])
        }
        Loop(name) => {
            let name_noun = term_to_noun(slab, name);
            T(slab, &[D(tas!(b"loop")), name_noun])
        }
        Made((name, args), s) => {
            let name_noun = term_to_noun(slab, name);
            let args_vec: Vec<_> = args.iter().map(|a| term_to_noun(slab, a)).collect();
            let args_noun = list_to_noun(slab, args_vec);
            let s_noun = spec_to_noun(slab, s);
            let inner = T(slab, &[name_noun, args_noun]);
            T(slab, &[D(tas!(b"made")), inner, s_noun])
        }
        Make(hoon, specs) => {
            let hoon_noun = hoon_to_noun(slab, hoon);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"make")), hoon_noun, specs_noun])
        }
        Name(name, s) => {
            let name_noun = term_to_noun(slab, name);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"name")), name_noun, s_noun])
        }
        Over(wing, s) => {
            let wing_noun = wing_to_noun(slab, wing);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"over")), wing_noun, s_noun])
        }
        BucGar(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bcgr")), a_noun, b_noun])
        }
        BucBuc(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bcbc")), a_noun, map_noun])
        }
        BucBar(a, h) => {
            let a_noun = spec_to_noun(slab, a);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bcbr")), a_noun, h_noun])
        }
        BucCab(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bccb")), h_noun])
        }
        BucCol(a, specs) => {
            let a_noun = spec_to_noun(slab, a);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"bccl")), a_noun, specs_noun])
        }
        BucCen(a, specs) => {
            let a_noun = spec_to_noun(slab, a);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"bccn")), a_noun, specs_noun])
        }
        BucDot(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bcdt")), a_noun, map_noun])
        }
        BucGal(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bcgl")), a_noun, b_noun])
        }
        BucHep(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bchp")), a_noun, b_noun])
        }
        BucKet(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bckt")), a_noun, b_noun])
        }
        BucLus(tag, s) => {
            let tag_noun = term_to_noun(slab, tag);
            let s_noun = spec_to_noun(slab, s);
            T(slab, &[D(tas!(b"bcls")), tag_noun, s_noun])
        }
        BucFas(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bcfs")), a_noun, map_noun])
        }
        BucMic(h) => {
            let inner = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bcmc")), inner])
        }
        BucPam(a, h) => {
            let a_noun = spec_to_noun(slab, a);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"bcpm")), a_noun, h_noun])
        }
        BucSig(h, a) => {
            let h_noun = hoon_to_noun(slab, h);
            let a_noun = spec_to_noun(slab, a);
            T(slab, &[D(tas!(b"bcsg")), h_noun, a_noun])
        }
        BucTic(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bctc")), a_noun, map_noun])
        }
        BucTis(skin, a) => {
            let skin_noun = skin_to_noun(slab, skin);
            let a_noun = spec_to_noun(slab, a);
            T(slab, &[D(tas!(b"bcts")), skin_noun, a_noun])
        }
        BucPat(a, b) => {
            let a_noun = spec_to_noun(slab, a);
            let b_noun = spec_to_noun(slab, b);
            T(slab, &[D(tas!(b"bcpt")), a_noun, b_noun])
        }
        BucWut(a, specs) => {
            let a_noun = spec_to_noun(slab, a);
            let specs_vec: Vec<_> = specs.iter().map(|s| spec_to_noun(slab, s)).collect();
            let specs_noun = list_to_noun(slab, specs_vec);
            T(slab, &[D(tas!(b"bcwt")), a_noun, specs_noun])
        }
        BucZap(a, map) => {
            let a_noun = spec_to_noun(slab, a);
            let entries: Vec<_> = map
                .iter()
                .map(|(k, v)| (term_to_noun(slab, k), spec_to_noun(slab, v)))
                .collect();
            let map_noun = map_to_noun(slab, entries);
            T(slab, &[D(tas!(b"bczp")), a_noun, map_noun])
        }
    }
}

fn skin_to_noun(slab: &mut NounSlab, skin: &Skin) -> Noun {
    use Skin::*;
    match skin {
        Term(s) => term_to_noun(slab, s),
        Base(bt) => {
            let inner = basetype_to_noun(slab, bt);
            T(slab, &[D(tas!(b"base")), inner])
        }
        Cell(l, r) => {
            let l = skin_to_noun(slab, l);
            let r = skin_to_noun(slab, r);
            T(slab, &[D(tas!(b"cell")), l, r])
        }
        Dbug(spot, s) => {
            let spot_noun = spot_to_noun(slab, spot);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"dbug")), spot_noun, s_noun])
        }
        Leaf(tag, atom) => {
            let tag_noun = cord_to_noun(slab, tag);
            let atom_noun = atom_to_noun(slab, atom);
            T(slab, &[D(tas!(b"leaf")), tag_noun, atom_noun])
        }
        Name(name, s) => {
            let name_noun = term_to_noun(slab, name);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"name")), name_noun, s_noun])
        }
        Over(wing, s) => {
            let wing_noun = wing_to_noun(slab, wing);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"over")), wing_noun, s_noun])
        }
        Spec(spec, s) => {
            let spec_noun = spec_to_noun(slab, spec);
            let s_noun = skin_to_noun(slab, s);
            T(slab, &[D(tas!(b"spec")), spec_noun, s_noun])
        }
        Wash(n) => T(slab, &[D(tas!(b"wash")), D(*n)]),
    }
}

fn wing_to_noun(slab: &mut NounSlab, wing: &WingType) -> Noun {
    let limbs: Vec<Noun> = wing.iter().map(|l| limb_to_noun(slab, l)).collect();

    list_to_noun(slab, limbs)
}

fn limb_to_noun(slab: &mut NounSlab, limb: &Limb) -> Noun {
    match limb {
        Limb::Term(s) => term_to_noun(slab, s),

        Limb::Axis(n) => T(slab, &[D(0), D(*n)]),

        Limb::Parent(n, opt) => {
            let opt_noun = match opt {
                Some(s) => {
                    let s_noun = term_to_noun(slab, s);
                    T(slab, &[D(0), s_noun])
                }
                None => D(0),
            };

            T(slab, &[D(1), D(*n), opt_noun])
        }
    }
}

fn spot_to_noun(slab: &mut NounSlab, spot: &Spot) -> Noun {
    let path_noun = path_to_noun(slab, &spot.p);
    let pint_noun = pint_to_noun(slab, &spot.q);
    T(slab, &[path_noun, pint_noun])
}

fn pint_to_noun(slab: &mut NounSlab, pint: &Pint) -> Noun {
    let p = T(slab, &[D(pint.p.0), D(pint.p.1)]);
    let q = T(slab, &[D(pint.q.0), D(pint.q.1)]);
    T(slab, &[p, q])
}

fn note_to_noun(slab: &mut NounSlab, note: &Note) -> Noun {
    match note {
        Note::Know(s) => {
            let s_noun = term_to_noun(slab, s);
            T(slab, &[D(tas!(b"know")), s_noun])
        }

        Note::Made(s, opt_wings) => {
            let s_noun = term_to_noun(slab, s);

            let wings_noun = opt_wings.as_ref().map(|wings| {
                let wing_nouns: Vec<Noun> = wings.iter().map(|w| wing_to_noun(slab, w)).collect();

                list_to_noun(slab, wing_nouns)
            });

            let wings_noun = match wings_noun {
                None => D(0),
                Some(p) => T(slab, &[D(0), p]),
            };

            T(slab, &[D(tas!(b"made")), s_noun, wings_noun])
        }
    }
}

fn woof_to_noun(slab: &mut NounSlab, woof: &Woof) -> Noun {
    match woof {
        Woof::ParsedAtom(a) => {
            let val = atom_to_noun(slab, a);
            val
        }
        Woof::Hoon(h) => {
            let val = hoon_to_noun(slab, h);
            T(slab, &[D(0), val])
        }
    }
}

fn tome_to_noun(slab: &mut NounSlab, tome: &Tome) -> Noun {
    // let what = term_to_noun(slab, tome.0); // unused
    let pairs: Vec<_> = tome
        .1
        .iter()
        .map(|(k, v)| (term_to_noun(slab, k), hoon_to_noun(slab, v)))
        .collect();
    let map = map_to_noun(slab, pairs);
    T(slab, &[D(0), map])
}

fn alas_to_noun(slab: &mut NounSlab, alas: &Alas) -> Noun {
    let pairs: Vec<_> = alas
        .iter()
        .map(|(k, v)| {
            let k_noun = term_to_noun(slab, k);
            let v_noun = hoon_to_noun(slab, v);
            (k_noun, v_noun)
        })
        .collect();
    map_to_noun(slab, pairs)
}

fn tyre_to_noun(slab: &mut NounSlab, tyre: &Tyre) -> Noun {
    let pairs: Vec<Noun> = tyre
        .iter()
        .map(|(k, v)| {
            let k_noun = term_to_noun(slab, k);
            let v_noun = hoon_to_noun(slab, v);
            T(slab, &[k_noun, v_noun])
        })
        .collect();
    list_to_noun(slab, pairs)
}

fn chum_to_noun(slab: &mut NounSlab, chum: &Chum) -> Noun {
    match chum {
        Chum::Lef(s) => term_to_noun(slab, s),
        Chum::StdKel(s, a) => {
            let s_noun = term_to_noun(slab, s);
            let a_noun = atom_to_noun(slab, a);
            T(slab, &[s_noun, a_noun])
        }
        Chum::VenProKel(v, p, a) => {
            let v_noun = term_to_noun(slab, v);
            let p_noun = term_to_noun(slab, p);
            let a_noun = atom_to_noun(slab, a);
            T(slab, &[v_noun, p_noun, a_noun])
        }
        Chum::VenProVerKel(v, p, a1, a2) => {
            let v_noun = term_to_noun(slab, v);
            let p_noun = term_to_noun(slab, p);
            let a1_noun = atom_to_noun(slab, a1);
            let a2_noun = atom_to_noun(slab, a2);
            T(slab, &[v_noun, p_noun, a1_noun, a2_noun])
        }
    }
}

fn nock_to_noun(slab: &mut NounSlab, nock: &Nock) -> Noun {
    use Nock::*;
    match nock {
        Pair(a, b) => {
            let a_noun = nock_to_noun(slab, a);
            let b_noun = nock_to_noun(slab, b);
            T(slab, &[D(2u64), a_noun, b_noun])
        }
        Const(expr) => {
            let expr_noun = noun_expr_to_noun(slab, expr);
            T(slab, &[D(1u64), expr_noun])
        }
        Compose(f, g) => {
            let f_noun = nock_to_noun(slab, f);
            let g_noun = nock_to_noun(slab, g);
            T(slab, &[D(7u64), f_noun, g_noun])
        }
        CellTest(n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(3u64), n_noun])
        }
        Increment(n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(4u64), n_noun])
        }
        Equality(a, b) => {
            let a_noun = nock_to_noun(slab, a);
            let b_noun = nock_to_noun(slab, b);
            T(slab, &[D(5u64), a_noun, b_noun])
        }
        IfThenElse(cond, yes, no) => {
            let cond_noun = nock_to_noun(slab, cond);
            let yes_noun = nock_to_noun(slab, yes);
            let no_noun = nock_to_noun(slab, no);
            T(slab, &[D(6u64), cond_noun, yes_noun, no_noun])
        }
        Edit((axis, new), core) => {
            let new_noun = nock_to_noun(slab, new);
            let core_noun = nock_to_noun(slab, core);
            let axis_cell = T(slab, &[D(*axis), new_noun]);
            T(slab, &[D(11u64), axis_cell, core_noun])
        }
        Hint(hint, n) => {
            let hint_noun = nock_hint_to_noun(slab, hint);
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(12u64), hint_noun, n_noun])
        }
        SerialCompose(f, g) => {
            let f = nock_to_noun(slab, f);
            let g = nock_to_noun(slab, g);
            T(slab, &[D(8u64), f, g])
        }
        PushSubject(n, subj) => {
            let n = nock_to_noun(slab, n);
            let subj = nock_to_noun(slab, subj);
            T(slab, &[D(9u64), n, subj])
        }
        SelectArm(axis, core) => {
            let core = nock_to_noun(slab, core);
            T(slab, &[D(10u64), D(*axis), core])
        }
        GrabData(core, path) => {
            let core = nock_to_noun(slab, core);
            let path = nock_to_noun(slab, path);
            T(slab, &[D(13u64), core, path])
        }
        AxisSelect(axis) => D(*axis),
    }
}

fn nock_hint_to_noun(slab: &mut NounSlab, hint: &NockHint) -> Noun {
    match hint {
        NockHint::ParsedAtom(a) => D(*a),
        NockHint::Pair(tag, n) => {
            let n_noun = nock_to_noun(slab, n);
            T(slab, &[D(*tag), n_noun])
        }
    }
}

fn term_or_tune_to_noun(slab: &mut NounSlab, tot: &TermOrTune) -> Noun {
    match tot {
        TermOrTune::Term(s) => term_to_noun(slab, s),
        TermOrTune::Tune(tune) => tune_to_noun(slab, tune),
    }
}

fn tune_to_noun(slab: &mut NounSlab, (map, vec): &Tune) -> Noun {
    let map_pairs: Vec<_> = map
        .iter()
        .map(|(k, opt_v)| {
            let k_noun = term_to_noun(slab, k);
            let v_noun = if let Some(v) = opt_v {
                hoon_to_noun(slab, v)
            } else {
                D(0)
            };
            (k_noun, v_noun)
        })
        .collect();

    let map_noun = map_to_noun(slab, map_pairs);

    let vec_nouns: Vec<_> = vec.iter().map(|v| hoon_to_noun(slab, v)).collect();

    let vec_noun = list_to_noun(slab, vec_nouns);

    T(slab, &[map_noun, vec_noun])
}

fn term_or_pair_to_noun(slab: &mut NounSlab, top: &TermOrPair) -> Noun {
    match top {
        TermOrPair::Term(s) => term_to_noun(slab, s),
        TermOrPair::Pair(s, h) => {
            let s_noun = term_to_noun(slab, s);
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[s_noun, h_noun])
        }
    }
}

fn zpwt_arg_to_noun(slab: &mut NounSlab, arg: &ZpwtArg) -> Noun {
    match arg {
        ZpwtArg::ParsedAtom(s) => {
            let tag = D(tas!(b"atom"));
            let s_noun = cord_to_noun(slab, s);
            T(slab, &[tag, s_noun])
        }
        ZpwtArg::Pair(s1, s2) => {
            let tag = D(tas!(b"pair"));
            let s1_noun = cord_to_noun(slab, s1);
            let s2_noun = cord_to_noun(slab, s2);
            T(slab, &[tag, s1_noun, s2_noun])
        }
    }
}

fn mane_to_noun(slab: &mut NounSlab, mane: &Mane) -> Noun {
    match mane {
        Mane::Tag(s) => term_to_noun(slab, s),
        Mane::TagSpace(s1, s2) => {
            let s1_noun = term_to_noun(slab, s1);
            let s2_noun = term_to_noun(slab, s2);
            T(slab, &[s1_noun, s2_noun])
        }
    }
}

fn marx_to_noun(slab: &mut NounSlab, marx: &Marx) -> Noun {
    let n = mane_to_noun(slab, &marx.n);
    let a = mart_to_noun(slab, &marx.a);
    T(slab, &[n, a])
}

fn manx_to_noun(slab: &mut NounSlab, manx: &Manx) -> Noun {
    let g = marx_to_noun(slab, &manx.g);
    let c = marl_to_noun(slab, &manx.c);
    T(slab, &[g, c])
}

fn mart_to_noun(slab: &mut NounSlab, mart: &Mart) -> Noun {
    let cells: Vec<Noun> = mart
        .iter()
        .map(|(mane, beers)| {
            let mane_noun = mane_to_noun(slab, mane);

            let beer_nouns: Vec<Noun> = beers.iter().map(|b| beer_to_noun(slab, b)).collect();

            let beers_noun = list_to_noun(slab, beer_nouns);

            T(slab, &[mane_noun, beers_noun])
        })
        .collect();

    list_to_noun(slab, cells)
}

fn beer_to_noun(slab: &mut NounSlab, beer: &Beer) -> Noun {
    match beer {
        Beer::Char(cord) => cord_to_noun(slab, cord),
        Beer::Hoon(h) => {
            let hoon_noun = hoon_to_noun(slab, h);
            T(slab, &[D(0), hoon_noun])
        }
    }
}

fn marl_to_noun(slab: &mut NounSlab, marl: &Marl) -> Noun {
    let items: Vec<Noun> = marl.iter().map(|t| tuna_to_noun(slab, t)).collect();

    list_to_noun(slab, items)
}

fn tuna_to_noun(slab: &mut NounSlab, tuna: &Tuna) -> Noun {
    match tuna {
        Tuna::Manx(m) => manx_to_noun(slab, m),
        Tuna::TunaTail(tail) => tuna_tail_to_noun(slab, tail),
    }
}

fn tuna_tail_to_noun(slab: &mut NounSlab, tail: &TunaTail) -> Noun {
    match tail {
        TunaTail::Tape(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"tape")), h_noun])
        }
        TunaTail::Manx(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"manx")), h_noun])
        }
        TunaTail::Marl(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"marl")), h_noun])
        }
        TunaTail::Call(h) => {
            let h_noun = hoon_to_noun(slab, h);
            T(slab, &[D(tas!(b"call")), h_noun])
        }
    }
}

// pub fn lth_b(slab: &mut NounSlab, a: Atom, b: Atom) -> bool {
//     if let (Ok(a), Ok(b)) = (a.as_direct(), b.as_direct()) {
//         a.data() < b.data()
//     } else if a.bit_size() > b.bit_size() {
//         false
//     } else if a.bit_size() < b.bit_size() {
//         true
//     } else {
//         a.as_ubig(stack) < b.as_ubig(stack)
//     }
// }

// pub fn lth(slab: &mut NounSlab, a: Atom, b: Atom) -> Noun {
//     if lth_b(stack, a, b) {
//         YES
//     } else {
//         NO
//     }
// }

pub fn dor_slab(a: Noun, b: Noun) -> Noun {
    if unsafe { a.raw_equals(&b) } {
        YES
    } else {
        match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
            (Left(atom_a), Left(atom_b)) => atom_less_than(atom_a, atom_b),
            (Left(_), Right(_)) => YES,
            (Right(_), Left(_)) => NO,
            (Right(cell_a), Right(cell_b)) => {
                let a_head = match slot(cell_a.as_noun(), 2) {
                    Ok(n) => n,
                    Err(_) => return NO,
                };
                let b_head = slot(cell_b.as_noun(), 2).unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                let a_tail = slot(cell_a.as_noun(), 3).unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                let b_tail = slot(cell_b.as_noun(), 3).unwrap_or_else(|err| {
                    panic!(
                        "Panicked with {err:?} at {}:{} (git sha: {:?})",
                        file!(),
                        line!(),
                        option_env!("GIT_SHA")
                    )
                });
                if unsafe { a_head.raw_equals(&b_head) } {
                    dor_slab(a_tail, b_tail)
                } else {
                    dor_slab(a_head, b_head)
                }
            }
        }
    }
}

pub fn gor_slab(a: Noun, b: Noun) -> Noun {
    let c = unsafe { DirectAtom::new_unchecked(slab_mug(a) as u64) };
    let d = unsafe { DirectAtom::new_unchecked(slab_mug(b) as u64) };

    match c.data().cmp(&d.data()) {
        Ordering::Greater => NO,
        Ordering::Less => YES,
        Ordering::Equal => dor_slab(a, b),
    }
}

pub fn mor_slab(a: Noun, b: Noun) -> Noun {
    let c = unsafe { DirectAtom::new_unchecked(slab_mug(a) as u64) };
    let d = unsafe { DirectAtom::new_unchecked(slab_mug(b) as u64) };

    let e = unsafe { DirectAtom::new_unchecked(slab_mug(c.as_noun()) as u64) };
    let f = unsafe { DirectAtom::new_unchecked(slab_mug(d.as_noun()) as u64) };

    match e.data().cmp(&f.data()) {
        Ordering::Greater => NO,
        Ordering::Less => YES,
        Ordering::Equal => dor_slab(a, b),
    }
}

pub fn atom_less_than(a: Atom, b: Atom) -> Noun {
    if atom_less_than_b(&a, &b) {
        YES
    } else {
        NO
    }
}

fn atom_less_than_b(a: &Atom, b: &Atom) -> bool {
    if let (Some(a_direct), Some(b_direct)) = (a.direct(), b.direct()) {
        return unsafe { a_direct.data() < b_direct.data() };
    }

    let a_bits = a.bit_size();
    let b_bits = b.bit_size();

    if a_bits != b_bits {
        return a_bits < b_bits;
    }

    if a_bits <= 64 && b_bits <= 64 {
        if let (Ok(a_u64), Ok(b_u64)) = (a.as_u64(), b.as_u64()) {
            return a_u64 < b_u64;
        }
    }

    let a_limbs = if a.is_direct() {
        &[unsafe { a.direct().unwrap().data() }]
    } else {
        unsafe { std::slice::from_raw_parts(a.data_pointer(), a.size()) }
    };

    let b_limbs = if b.is_direct() {
        &[unsafe { b.direct().unwrap().data() }]
    } else {
        unsafe { std::slice::from_raw_parts(b.data_pointer(), b.size()) }
    };

    let max_limbs = a_limbs.len().max(b_limbs.len());
    for i in 0..max_limbs {
        let a_idx = a_limbs.len().wrapping_sub(1).wrapping_sub(i);
        let b_idx = b_limbs.len().wrapping_sub(1).wrapping_sub(i);

        let a_limb = if a_idx < a_limbs.len() {
            a_limbs[a_idx]
        } else {
            0
        };
        let b_limb = if b_idx < b_limbs.len() {
            b_limbs[b_idx]
        } else {
            0
        };

        if a_limb != b_limb {
            return a_limb < b_limb;
        }
    }

    false
}

pub fn collect_inputs(path: &PathBuf) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_inputs_inner(path, &mut files);
    files.sort();
    files
}

fn collect_inputs_inner(path: &PathBuf, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) == Some("hoon") {
            out.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        let entries = std::fs::read_dir(path).unwrap_or_else(|e| {
            eprintln!("Failed to read directory '{}': {}", path.display(), e);
            std::process::exit(1);
        });

        for entry in entries {
            let entry = entry.unwrap_or_else(|e| {
                eprintln!(
                    "Failed to read directory entry in '{}': {}",
                    path.display(),
                    e
                );
                std::process::exit(1);
            });

            collect_inputs_inner(&entry.path(), out);
        }
    } else {
        eprintln!("Invalid input path: {}", path.display());
        std::process::exit(2);
    }
}
