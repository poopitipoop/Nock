use std::collections::*;
use std::sync::Arc;

use chumsky::input::{Stream, ValueInput};
use chumsky::prelude::*;

use crate::ast::hoon::*;
use crate::utils::*;

pub fn fas_runes_tall<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
    wer: Path,
    linemap: Arc<LineMap>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    choice((
        just('=').ignore_then(fastis(hoon.clone(), hoon_wide.clone())),
        just('*').ignore_then(fastar(hoon.clone(), hoon_wide.clone())),
        just('#').ignore_then(fashax(hoon.clone(), hoon_wide.clone())),
    ))
    .boxed()
}

pub fn fastis<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}

pub fn fastar<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}

pub fn fashax<'src>(
    hoon: impl ParserExt<'src, Hoon>,
    hoon_wide: impl ParserExt<'src, Hoon>,
) -> impl Parser<'src, &'src str, Hoon, Err<'src>> {
    gap()
        .ignore_then(hoon_wide.clone())
        .ignore_then(gap())
        .ignore_then(hoon.clone())
}
