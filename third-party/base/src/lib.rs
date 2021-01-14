mod mmap;
mod errno;
mod external_mapping;

pub use crate::mmap::*;
pub use crate::errno::*;
pub use crate::external_mapping::*;

pub use crate::external_mapping::Error as ExternalMappingError;
pub use crate::external_mapping::Result as ExternalMappingResult;

use libc::{sysconf, _SC_PAGESIZE};

/// Safe wrapper for `sysconf(_SC_PAGESIZE)`.
#[inline(always)]
pub fn pagesize() -> usize {
    // Trivially safe
    unsafe { sysconf(_SC_PAGESIZE) as usize }
}


