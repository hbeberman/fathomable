use std::hint::black_box;
use std::time::{Duration, Instant};

#[inline(never)]
fn hot() -> u64 {
    let start = Instant::now();
    let mut value = 0_u64;
    while start.elapsed() < Duration::from_secs(2) {
        value = black_box(value.wrapping_add(1));
    }
    value
}

#[inline(never)]
fn descend(depth: usize, seed: u8) -> u64 {
    let mut frame = [seed; 4096];
    black_box(&mut frame);
    let result = if depth == 0 {
        hot()
    } else {
        descend(depth - 1, seed.wrapping_add(1))
    };
    black_box(&frame);
    result
}

fn main() {
    black_box(descend(12, 1));
}
