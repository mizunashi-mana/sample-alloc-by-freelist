use std::{error::Error, ptr::NonNull, mem::size_of};

mod sys {
    use std::{error::Error, ptr::NonNull};

    pub type AnyNonNull = NonNull<libc::c_void>;

    pub unsafe fn get_pagesize() -> Result<usize, Box<dyn Error>> {
        let pagesize = libc::sysconf(libc::_SC_PAGE_SIZE);
        if pagesize < 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(pagesize as usize)
        }
    }

    pub unsafe fn reserve(len: usize) -> Result<AnyNonNull, Box<dyn Error>> {
        let ptr = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_NONE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
            -1,
            0
        );
        if ptr == libc::MAP_FAILED {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(NonNull::new_unchecked(ptr))
        }
    }

    #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
    pub enum CommitStrategy {
        Mprotect,
        MmapFixed,
    }

    pub unsafe fn commit(
        addr: AnyNonNull,
        len: usize,
        prefer_strategy: CommitStrategy,
    ) -> Result<CommitStrategy, Box<dyn Error>> {
        if prefer_strategy <= CommitStrategy::Mprotect {
            // mprotect was added in Linux 4.9.
            let result = libc::mprotect(
                addr.as_ptr(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
            );
            if result == 0 {
                return Ok(CommitStrategy::Mprotect);
            }
        }

        // Remapping fixed regions is unrecommended.
        // Use as a fallback if we cannot use mprotect.
        let ptr = libc::mmap(
            addr.as_ptr(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_FIXED,
            -1,
            0,
        );
        if ptr == libc::MAP_FAILED {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(CommitStrategy::MmapFixed)
        }
    }

    #[allow(unused)]
    #[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
    pub enum DecommitStrategy {
        Mprotect,
        MmapFixed,
    }

    #[allow(unused)]
    pub unsafe fn decommit(
        addr: AnyNonNull,
        len: usize,
        prefer_strategy: DecommitStrategy
    ) -> Result<DecommitStrategy, Box<dyn Error>> {
        if prefer_strategy <= DecommitStrategy::Mprotect {
            // mprotect was added in Linux 4.9.
            let result = libc::mprotect(
                addr.as_ptr(),
                len,
                libc::PROT_NONE,
            );
            if result == 0 {
                return Ok(DecommitStrategy::Mprotect);
            }
        }

        // Remapping fixed regions is unrecommended.
        // Use as a fallback if we cannot use mprotect.
        let ptr = libc::mmap(
            addr.as_ptr(),
            len,
            libc::PROT_NONE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_FIXED,
            -1,
            0,
        );
        if ptr == libc::MAP_FAILED {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(DecommitStrategy::MmapFixed)
        }
    }

    pub unsafe fn alloc(len: usize) -> Result<AnyNonNull, Box<dyn Error>> {
        let ptr = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANONYMOUS | libc::MAP_PRIVATE,
            -1,
            0,
        );
        if ptr == libc::MAP_FAILED {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(NonNull::new_unchecked(ptr))
        }
    }

    pub unsafe fn release(addr: AnyNonNull, len: usize) -> Result<(), Box<dyn Error>> {
        let result = libc::munmap(addr.as_ptr(), len);
        if result != 0 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }
}

struct Header {
    size_or_class_of_subheap: usize,
}

const MAX_HEAP_SIZE: usize = 2 << 32;
const SUBHEAP_COUNT: usize = 7;

const fn block_size_of_subheap(class_of_subheap: usize) -> usize {
    2 << (class_of_subheap + 3)
}

/// * `alignment` - A power of 2.
const fn aligned_size(original: usize, alignment: usize) -> usize {
    let mask = alignment - 1;
    original + (original.reverse_bits() & mask)
}

const MAX_BLOCK_SIZE: usize = block_size_of_subheap(SUBHEAP_COUNT - 1);

pub struct Allocator {
    // immutable
    pagesize: usize,
    heap_end: sys::AnyNonNull,

    // mutable
    free_lists: [*mut FreeHeader; SUBHEAP_COUNT],
    active_heap_end: sys::AnyNonNull,
    commited_heap_end: sys::AnyNonNull,

    prefer_commit_strategy: sys::CommitStrategy,
}

struct FreeHeader {
    #[allow(unused)]
    header: Header,
    next: *mut FreeHeader,
}

impl Allocator {
    pub unsafe fn init() -> Result<Self, Box<dyn Error>> {
        let pagesize = sys::get_pagesize()?;
        assert!(MAX_HEAP_SIZE % pagesize == 0);

        let heap_begin = sys::reserve(MAX_HEAP_SIZE)?;
        let heap_end = NonNull::new_unchecked(heap_begin.as_ptr().add(MAX_HEAP_SIZE));
        let free_lists = [std::ptr::null_mut(); SUBHEAP_COUNT];

        Ok(Self {
            pagesize,
            heap_end,
            free_lists,
            active_heap_end: heap_begin,
            commited_heap_end: heap_begin,
            prefer_commit_strategy: sys::CommitStrategy::Mprotect,
        })
    }

    pub unsafe fn alloc<T: Sized>(&mut self) -> Result<NonNull<T>, Box<dyn Error>> {
        self.alloc_by_size(size_of::<T>())
    }

    pub unsafe fn free<T>(&mut self, ptr: NonNull<T>) -> Result<(), Box<dyn Error>> {
        let allocated_ptr = (ptr.as_ptr() as *mut libc::c_void)
            .offset(- (size_of::<Header>() as isize));
        let allocated_ptr = NonNull::new_unchecked(allocated_ptr as *mut Header);
        
        let size_or_class_of_subheap = allocated_ptr.as_ref().size_or_class_of_subheap;
        if size_or_class_of_subheap <= MAX_BLOCK_SIZE {
            let class_of_subheap = size_or_class_of_subheap;
            self.free_on_subheap(allocated_ptr, class_of_subheap)
        } else {
            let size = size_or_class_of_subheap;
            self.free_on_external(allocated_ptr, size)
        }
    }

    pub unsafe fn alloc_by_size<T>(&mut self, len: usize) -> Result<NonNull<T>, Box<dyn Error>> {
        if len <= MAX_BLOCK_SIZE {
            for class_of_subheap in 0..SUBHEAP_COUNT {
                if len <= block_size_of_subheap(class_of_subheap) {
                    return self.alloc_on_subheap(class_of_subheap);
                }
            }
            self.alloc_on_subheap(SUBHEAP_COUNT - 1)
        } else {
            self.alloc_on_external(len)
        }
    }

    unsafe fn alloc_on_subheap<T>(&mut self, class_of_subheap: usize) -> Result<NonNull<T>, Box<dyn Error>> {
        match NonNull::new(self.free_lists[class_of_subheap]) {
            None => {
                let allocated_ptr = self.extend_active_heap_end(class_of_subheap)?;
                let allocated_ptr: NonNull<libc::c_void> = allocated_ptr.cast();
                Ok(NonNull::new_unchecked(allocated_ptr.as_ptr().add(size_of::<Header>()) as *mut T))
            }
            Some(free_ptr) => {
                self.free_lists[class_of_subheap] = free_ptr.as_ref().next;
                let used_ptr: NonNull<libc::c_void> = free_ptr.cast();
                Ok(NonNull::new_unchecked(used_ptr.as_ptr().add(size_of::<Header>()) as *mut T))
            }
        }
    }

    unsafe fn free_on_subheap(&mut self, addr: NonNull<Header>, class_of_subheap: usize) -> Result<(), Box<dyn Error>> {
        let mut addr: NonNull<FreeHeader> = addr.cast();
        addr.as_mut().next = self.free_lists[class_of_subheap];
        self.free_lists[class_of_subheap] = addr.as_ptr();
        Ok(())
    }

    unsafe fn alloc_on_external<T>(&mut self, len: usize) -> Result<NonNull<T>, Box<dyn Error>> {
        let allocated_size = aligned_size(len + size_of::<Header>(), self.pagesize);
        let mut allocated_ptr: NonNull<Header> = sys::alloc(allocated_size)?.cast();
        *allocated_ptr.as_mut() = Header {
            size_or_class_of_subheap: allocated_size,
        };
        Ok(allocated_ptr.cast())
    }

    unsafe fn free_on_external(&mut self, addr: NonNull<Header>, size: usize) -> Result<(), Box<dyn Error>> {
        sys::release(addr.cast(), size)
    }

    unsafe fn extend_active_heap_end(&mut self, class_of_subheap: usize) -> Result<NonNull<Header>, Box<dyn Error>> {
        let allocated_size = size_of::<Header>() + block_size_of_subheap(class_of_subheap);
        let new_active_heap_end = NonNull::new_unchecked(self.active_heap_end.as_ptr().add(allocated_size));
        if self.heap_end < new_active_heap_end {
            return Err(format!("Failed to extend heap size.").into());
        }

        if self.commited_heap_end < new_active_heap_end {
            let committed_size = aligned_size(
                new_active_heap_end.as_ptr().offset_from(self.active_heap_end.as_ptr()) as usize,
                self.pagesize,
            );
            self.prefer_commit_strategy = sys::commit(self.commited_heap_end, committed_size, self.prefer_commit_strategy)?;
            self.commited_heap_end = NonNull::new_unchecked(self.commited_heap_end.as_ptr().add(committed_size));
        }

        let mut allocated_ptr: NonNull<Header> = self.active_heap_end.cast();
        self.active_heap_end = new_active_heap_end;

        *allocated_ptr.as_mut() = Header {
            size_or_class_of_subheap: class_of_subheap,
        };
        Ok(allocated_ptr)
    }
}