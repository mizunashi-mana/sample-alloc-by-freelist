mod alloc;

fn main() {
    unsafe { unsafe_main() }
}

unsafe fn unsafe_main() {
    let mut allocator = alloc::Allocator::init().unwrap();

    let ptr1: &mut i32 = allocator.alloc().unwrap().as_mut();
    *ptr1 = 111;

    let ptr2: &mut [i32; 10000] = allocator.alloc().unwrap().as_mut();
    ptr2.fill(222);

    println!("Check value: {:?}, {:?}", ptr1, ptr2[10]);

    let mut item1 = vec![1, 2, 3, 4, 5, 6];
    item1[2] = 333;

    let mut item2 = Vec::new();
    item2.resize(10000, 444);

    *ptr1 = 555;
    ptr2[10] = 666;

    println!("Check value: {:?}, {:?}, {:?}, {:?}", ptr1, ptr2[10], item1[2], item2[10]);

    allocator.free(ptr1.into()).unwrap();
    allocator.free(ptr2.into()).unwrap();

    println!("Check value: {:?}, {:?}", item1[2], item2[10]);

    let ptr1: &mut i32 = allocator.alloc().unwrap().as_mut();
    *ptr1 = 777;

    let ptr2: &mut [i32; 10000] = allocator.alloc().unwrap().as_mut();
    ptr2.fill(888);

    println!("Check value: {:?}, {:?}, {:?}, {:?}", ptr1, ptr2[10], item1[2], item2[10]);
}
