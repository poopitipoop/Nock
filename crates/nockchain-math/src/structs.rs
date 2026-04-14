use nockvm::jets::sort::util::gor;
use nockvm::mem::NockStack;
use nockvm::noun::{Cell, CellHandle, Noun, NounHandle, NounSpace};
use nockvm::unifying_equality::unifying_equality;

use crate::noun_ext::NounMathExt;

#[derive(Copy, Clone)]
pub struct HoonList<'a> {
    pub(super) next: Option<Noun>,
    pub(super) space: &'a NounSpace,
}

impl<'a> Iterator for HoonList<'a> {
    type Item = Noun;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        self.next.take().map(|noun| {
            let cell = noun.in_space(self.space).as_cell().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            let tail = cell.tail().noun();
            self.next = if tail.is_cell() { Some(tail) } else { None };
            cell.head().noun()
        })
    }
}

impl<'a> HoonList<'a> {
    pub fn try_from(
        n: Noun,
        space: &'a NounSpace,
    ) -> core::result::Result<Self, nockvm::noun::Error> {
        if n.is_cell() {
            let _ = n.in_space(space).as_cell().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            Ok(HoonList {
                next: Some(n),
                space,
            })
        } else {
            Ok(HoonList { next: None, space })
        }
    }

    pub fn from_cell(c: Cell, space: &'a NounSpace) -> Self {
        Self {
            next: Some(c.as_noun()),
            space,
        }
    }
}

pub fn next_cell(cell: Cell, space: &NounSpace) -> Option<Cell> {
    let cell_handle = CellHandle::new(cell, space);
    let tail = cell_handle.tail().noun();
    if tail.is_cell() {
        let handle = tail.in_space(space).as_cell().unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        Some(handle.cell())
    } else {
        None
    }
}

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub struct HoonMap<'a> {
    pub(super) node: Noun,
    pub(super) left: Option<Noun>,
    pub(super) right: Option<Noun>,
    pub(super) space: &'a NounSpace,
}

impl<'a> HoonMap<'a> {
    pub fn get(&self, stack: &mut NockStack, mut k: Noun) -> Option<Noun> {
        let [mut ck, cv] = self.node.uncell(self.space).ok()?;

        if unsafe { unifying_equality(stack, &mut ck, &mut k) } {
            // ?:  =(b p.n.a)
            //   (some q.n.a)
            Some(cv)
        } else if gor(stack, k, ck, self.space).as_direct().map(|v| v.data()) == Ok(0) {
            // ?:  (gor b p.n.a)
            //   $(a l.a)
            let map = Self::try_from(self.left?.in_space(&self.space)).ok()?;
            map.get(stack, k)
        } else {
            // $(a r.a)
            let map = Self::try_from(self.right?.in_space(&self.space)).ok()?;
            map.get(stack, k)
        }
    }

    pub fn try_from(n: NounHandle<'a>) -> std::result::Result<Self, nockvm::noun::Error> {
        if n.is_cell() {
            let cell = n.as_cell().unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            HoonMap::try_from_cell(cell)
        } else {
            not_cell()
        }
    }

    pub fn try_from_cell(c: CellHandle<'a>) -> std::result::Result<Self, nockvm::noun::Error> {
        let tail = c.tail();
        if let Ok(cell_tail) = tail.as_cell() {
            let left = cell_tail.head().noun();
            let right = cell_tail.tail().noun();

            Ok(Self {
                node: c.head().noun(),
                left: left.is_cell().then_some(left),
                right: right.is_cell().then_some(right),
                space: c.space(),
            })
        } else {
            not_cell()
        }
    }
}
#[allow(dead_code)]
#[derive(Clone)]
pub struct HoonMapIter<'a> {
    pub(super) stack: Vec<Option<NounHandle<'a>>>,
    pub(super) space: &'a NounSpace,
}

impl<'a> Iterator for HoonMapIter<'a> {
    type Item = NounHandle<'a>;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(maybe_cell) = self.stack.pop() {
            if let Some(noun) = maybe_cell {
                if let Ok(cell_trie) = HoonMap::try_from(noun) {
                    self.stack
                        .push(cell_trie.right.map(|n| n.in_space(&self.space)));
                    self.stack
                        .push(cell_trie.left.map(|n| n.in_space(&self.space)));
                    return Some(cell_trie.node.in_space(&self.space));
                } else {
                    return self.next();
                }
            } else {
                return self.next();
            }
        }
        None
    }
}
fn not_cell<T>() -> core::result::Result<T, nockvm::noun::Error> {
    Err(nockvm::noun::Error::NotCell)
}

impl<'a> HoonMapIter<'a> {
    pub fn new(n: &'a NounHandle) -> Self {
        if n.is_cell() {
            Self {
                stack: vec![Some(*n)],
                space: n.space(),
            }
        } else {
            Self {
                stack: vec![None],
                space: n.space(),
            }
        }
    }
}
