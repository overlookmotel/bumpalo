use bumpalo::Bump;
use std::alloc::Layout;
use std::fmt::Debug;
use std::mem;
use std::usize;

#[test]
fn can_iterate_over_allocated_things() {
    let mut bump = Bump::new();

    #[cfg(not(miri))]
    const MAX: u64 = 131_072;

    #[cfg(miri)] // Miri is very slow, pick a smaller max that runs in a reasonable amount of time
    const MAX: u64 = 1024;

    let mut chunk_ends = vec![];
    let mut last = None;

    for i in 0..MAX {
        let this = bump.alloc(i);
        assert_eq!(*this, i);
        let this = this as *const _ as usize;

        if match last {
            Some(last) if last - mem::size_of::<u64>() == this => false,
            _ => true,
        } {
            let chunk_end = this + mem::size_of::<u64>();
            println!("new chunk ending @ 0x{:x}", chunk_end);
            assert!(
                !chunk_ends.contains(&chunk_end),
                "should not have already allocated this chunk"
            );
            chunk_ends.push(chunk_end);
        }

        last = Some(this);
    }

    let mut seen = vec![false; MAX as usize];

    // Safe because we always allocated objects of the same type in this arena,
    // and their size >= their align.
    for ch in bump.iter_allocated_chunks() {
        let chunk_end = ch.as_ptr() as usize + ch.len();
        println!("iter chunk ending @ {:#x}", chunk_end);
        assert_eq!(
            chunk_ends.pop().unwrap(),
            chunk_end,
            "should iterate over each chunk once, in order they were allocated in"
        );

        let (before, mid, after) = unsafe { ch.align_to::<u64>() };
        assert!(before.is_empty());
        assert!(after.is_empty());
        for i in mid {
            assert!(*i < MAX, "{} < {} (aka {:x} < {:x})", i, MAX, i, MAX);
            seen[*i as usize] = true;
        }
    }

    assert!(seen.iter().all(|s| *s));
}

#[cfg(not(miri))] // Miri does not panic on OOM, the interpreter halts
#[test]
#[should_panic(expected = "out of memory")]
fn oom_instead_of_bump_pointer_overflow() {
    let bump = Bump::new();
    let x = bump.alloc(0_u8);
    let p = x as *mut u8 as usize;

    // A size guaranteed to overflow the bump pointer.
    let size = (isize::MAX as usize) - p + 1;
    let align = 1;
    let layout = match Layout::from_size_align(size, align) {
        Err(e) => {
            // Return on error so that we don't panic and the test fails.
            eprintln!("Layout::from_size_align errored: {}", e);
            return;
        }
        Ok(l) => l,
    };

    // This should panic.
    bump.alloc_layout(layout);
}

#[test]
fn force_new_chunk_fits_well() {
    let b = Bump::new();

    // Use the first chunk for something
    b.alloc_layout(Layout::from_size_align(1, 1).unwrap());

    // Next force allocation of some new chunks.
    b.alloc_layout(Layout::from_size_align(100_001, 1).unwrap());
    b.alloc_layout(Layout::from_size_align(100_003, 1).unwrap());
}

#[test]
fn alloc_with_strong_alignment() {
    let b = Bump::new();

    // 64 is probably the strongest alignment we'll see in practice
    // e.g. AVX-512 types, or cache line padding optimizations
    b.alloc_layout(Layout::from_size_align(4096, 64).unwrap());
}

#[test]
fn alloc_slice_copy() {
    let b = Bump::new();

    let src: &[u16] = &[0xFEED, 0xFACE, 0xA7, 0xCAFE];
    let dst = b.alloc_slice_copy(src);

    assert_eq!(src, dst);
}

#[test]
fn alloc_slice_clone() {
    let b = Bump::new();

    let src = vec![vec![0], vec![1, 2], vec![3, 4, 5], vec![6, 7, 8, 9]];
    let dst = b.alloc_slice_clone(&src);

    assert_eq!(src, dst);
}

#[test]
fn small_size_and_large_align() {
    let b = Bump::new();
    let layout = std::alloc::Layout::from_size_align(1, 0x1000).unwrap();
    b.alloc_layout(layout);
}

fn with_capacity_helper<I, T>(iter: I)
where
    T: Copy + Debug + Eq,
    I: Clone + Iterator<Item = T> + DoubleEndedIterator,
{
    for &initial_size in &[0, 1, 8, 11, 0x1000, 0x12345] {
        let mut b = Bump::<1>::with_min_align_and_capacity(initial_size);

        for v in iter.clone() {
            b.alloc(v);
        }

        let mut pushed_values = b.iter_allocated_chunks().flat_map(|c| {
            let (before, mid, after) = unsafe { c.align_to::<T>() };
            assert!(before.is_empty());
            assert!(after.is_empty());
            mid.iter().copied()
        });

        let mut iter = iter.clone().rev();
        for (expected, actual) in iter.by_ref().zip(pushed_values.by_ref()) {
            assert_eq!(expected, actual);
        }

        assert!(iter.next().is_none());
        assert!(pushed_values.next().is_none());
    }
}

#[test]
fn with_capacity_test() {
    with_capacity_helper(0u8..255);
    #[cfg(not(miri))] // Miri is very slow, disable most of the test cases when using it
    {
        with_capacity_helper(0u16..10000);
        with_capacity_helper(0u32..10000);
        with_capacity_helper(0u64..10000);
        with_capacity_helper(0u128..10000);
    }
}

#[test]
fn test_reset() {
    let mut b = Bump::new();

    for i in 0u64..10_000 {
        b.alloc(i);
    }

    assert!(b.iter_allocated_chunks().count() > 1);

    let last_chunk = b.iter_allocated_chunks().next().unwrap();
    let start = last_chunk.as_ptr() as usize;
    let end = start + last_chunk.len();
    b.reset();
    assert_eq!(
        end - mem::size_of::<u64>(),
        b.alloc(0u64) as *const u64 as usize
    );
    assert_eq!(b.iter_allocated_chunks().count(), 1);
}

#[test]
fn test_alignment() {
    for &alignment in &[2, 4, 8, 16, 32, 64] {
        let b = Bump::with_capacity(513);
        let layout = std::alloc::Layout::from_size_align(alignment, alignment).unwrap();

        for _ in 0..1024 {
            let ptr = b.alloc_layout(layout).as_ptr();
            assert_eq!(ptr as *const u8 as usize % alignment, 0);
        }
    }
}

#[test]
fn test_chunk_capacity() {
    let b = Bump::with_capacity(512);
    let orig_capacity = b.chunk_capacity();
    b.alloc(true);
    assert!(b.chunk_capacity() < orig_capacity);
}

#[test]
#[cfg(feature = "allocator_api")]
fn miri_stacked_borrows_issue_247() {
    let bump = bumpalo::Bump::new();

    let a = Box::into_raw(Box::new_in(1u8, &bump));
    drop(unsafe { Box::from_raw_in(a, &bump) });

    let _b = Box::new_in(2u16, &bump);
}