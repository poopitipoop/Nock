#![feature(negative_impls)]
#![feature(slice_pattern)]
// Allow unwrap in test code - standard practice for test assertions
#![cfg_attr(test, allow(clippy::unwrap_used))]

//! # Crown
//!
//! The Crown library provides a set of modules and utilities for working with
//! the Sword runtime. It includes functionality for handling jammed nouns, kernels (as jammed nouns),
//! and various types and utilities that make nockvm easier to use.
//!
//! ## Modules
//!
//! - `kernel`: Sword runtime interface.
//! - `noun`: Extensions and utilities for working with Urbit nouns.
//! - `utils`: Errors, misc functions and extensions.
//!
pub mod drivers;
pub(crate) mod event_log;
pub mod kernel;
pub mod nockapp;
pub mod noun;
pub mod observability;
pub(crate) mod snapshot;
pub mod utils;

use std::path::PathBuf;

pub use bytes::*;
pub use drivers::*;
pub use nockapp::*;
pub use nockvm::noun::Noun;
pub use noun::{AtomExt, IndirectAtomExt, JammedNoun, NounExt};
pub use utils::bytes::{ToBytes, ToBytesExt};
pub use utils::error::{CrownError, Result};

/// Returns the default directory where kernel data is stored.
///
/// # Arguments
///
/// * `dir` - A string slice that holds the kernel identifier.
///
/// # Example
///
/// ```
///
/// use std::path::PathBuf;
/// use nockapp::default_data_dir;
/// let dir = default_data_dir("nockapp");
/// assert_eq!(dir, PathBuf::from("./.data.nockapp"));
/// ```
pub fn default_data_dir(dir_name: &str) -> PathBuf {
    PathBuf::from(format!("./.data.{}", dir_name))
}

pub fn system_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NOCKAPP_HOME") {
        if !dir.trim().is_empty() {
            let path = PathBuf::from(&dir);
            if path.is_absolute() {
                return path;
            }
            if let Ok(current) = std::env::current_dir() {
                return current.join(path);
            }
            return PathBuf::from(dir);
        }
    }

    let home_dir = dirs::home_dir().expect("Failed to get home directory");
    home_dir.join(".nockapp")
}

/// Default size for the Nock stack (1 GB)
pub const DEFAULT_NOCK_STACK_SIZE: usize = 1 << 27;

#[cfg(test)]
pub mod test_support {
    use std::cell::Cell;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use nockvm::mem::NockStack;

    pub struct NativePmaTestGuard {
        _guard: Option<MutexGuard<'static, ()>>,
    }

    pub struct TestArena {
        _guard: NativePmaTestGuard,
        stack: NockStack,
    }

    impl TestArena {
        pub fn with_words(words: usize) -> Self {
            let guard = native_pma_test_guard();
            let stack = NockStack::new(words, 0);
            Self {
                _guard: guard,
                stack,
            }
        }
    }

    impl Default for TestArena {
        fn default() -> Self {
            // A modest stack is enough because tests mostly need TLS to be populated.
            Self::with_words(1 << 16)
        }
    }

    impl std::ops::Deref for TestArena {
        type Target = NockStack;

        fn deref(&self) -> &Self::Target {
            &self.stack
        }
    }

    impl std::ops::DerefMut for TestArena {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.stack
        }
    }

    thread_local! {
        static NATIVE_PMA_TEST_GUARD_DEPTH: Cell<usize> = const { Cell::new(0) };
    }

    impl Drop for NativePmaTestGuard {
        fn drop(&mut self) {
            NATIVE_PMA_TEST_GUARD_DEPTH.with(|depth| {
                let current = depth.get();
                depth.set(current.saturating_sub(1));
            });
        }
    }

    pub fn native_pma_test_guard() -> NativePmaTestGuard {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = NATIVE_PMA_TEST_GUARD_DEPTH.with(|depth| {
            let current = depth.get();
            depth.set(current.saturating_add(1));
            if current == 0 {
                Some(
                    LOCK.get_or_init(|| Mutex::new(()))
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                )
            } else {
                None
            }
        });
        NativePmaTestGuard { _guard: guard }
    }
}
