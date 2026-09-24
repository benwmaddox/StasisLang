use std::env;
use std::hint::black_box;

fn fib(n: i32) -> i32 {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn mandelbrot() -> i32 {
    let mut total = 0;
    for _pass in 0..15 {
        for y in 0..240 {
            let ci = (y as f64) * 2.0 / 240.0 - 1.0;
            for x in 0..320 {
                let cr = (x as f64) * 3.0 / 320.0 - 2.0;
                let (mut zr, mut zi) = (0.0, 0.0);
                for steps in 0..50 {
                    if zr * zr + zi * zi > 4.0 {
                        break;
                    }
                    let next = zr * zr - zi * zi + cr;
                    zi = 2.0 * zr * zi + ci;
                    zr = next;
                    if steps == 49 {
                        total += 1;
                    }
                }
            }
        }
    }
    total
}

fn matmul() -> i32 {
    let mut a = vec![0_i32; 4096];
    let mut b = vec![0_i32; 4096];
    let mut c = vec![0_i32; 4096];
    for i in 0..64 {
        for j in 0..64 {
            a[i * 64 + j] = ((i + j) % 17) as i32;
        }
    }
    for pass in 0..100 {
        for i in 0..64 {
            for j in 0..64 {
                b[i * 64 + j] = ((i * 3 + j + pass) % 19) as i32;
            }
        }
        for i in 0..64 {
            for j in 0..64 {
                let mut sum = 0;
                for k in 0..64 {
                    sum += a[i * 64 + k] * b[k * 64 + j];
                }
                c[i * 64 + j] = sum;
            }
        }
        black_box(&c);
    }
    (0..64).map(|i| c[i * 64 + i]).sum()
}

fn solve_queens(row: usize, columns: &mut [i32; 12]) -> i32 {
    if row == 12 {
        return 1;
    }
    let mut count = 0;
    for col in 0..12_i32 {
        let mut allowed = true;
        for prior in 0..row {
            let distance = (col - columns[prior]).abs();
            if columns[prior] == col || distance == (row - prior) as i32 {
                allowed = false;
            }
        }
        if allowed {
            columns[row] = col;
            count += solve_queens(row + 1, columns);
        }
    }
    count
}

fn sieve() -> i32 {
    let mut composite = vec![0_u8; 100000];
    let mut count = 0;
    for _pass in 0..300 {
        composite.fill(0);
        count = 0;
        for p in 2..100000 {
            if composite[p] == 0 {
                count += 1;
                if p <= 316 {
                    for multiple in (p * p..100000).step_by(p) {
                        composite[multiple] = 1;
                    }
                }
            }
        }
        black_box(&composite);
    }
    count
}

fn partial_sums() -> i32 {
    let (mut total, mut weighted) = (0.0_f64, 0.0_f64);
    for i in 1..=100_000_000 {
        let term = 1.0 / ((i as f64) * (i as f64));
        total += term;
        weighted += total;
    }
    i32::from(
        total > 1.6449 && total < 1.6450 && weighted > 164_000_000.0 && weighted < 165_000_000.0,
    )
}

fn life() -> i32 {
    let mut cells = vec![0_u8; 16384];
    let mut next_cells = vec![0_u8; 16384];
    for y in 0..128 {
        for x in 0..128 {
            if (x * 17 + y * 31 + x * y) % 11 < 4 {
                cells[y * 128 + x] = 1;
            }
        }
    }
    for _generation in 0..100 {
        for y in 0..128 {
            for x in 0..128 {
                let mut neighbors = 0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx != 0 || dy != 0 {
                            let nx = ((x as i32 + dx + 128) % 128) as usize;
                            let ny = ((y as i32 + dy + 128) % 128) as usize;
                            neighbors += cells[ny * 128 + nx] as i32;
                        }
                    }
                }
                let index = y * 128 + x;
                next_cells[index] =
                    u8::from(neighbors == 3 || (cells[index] != 0 && neighbors == 2));
            }
        }
        cells.copy_from_slice(&next_cells);
    }
    cells.into_iter().map(i32::from).sum()
}

fn main() {
    let name = env::args().nth(1).expect("benchmark name required");
    let value = match name.as_str() {
        "fib" => fib(38),
        "mandelbrot" => mandelbrot(),
        "matmul" => matmul(),
        "nqueens" => solve_queens(0, &mut [0; 12]),
        "sieve" => sieve(),
        "partial_sums" => partial_sums(),
        "life" => life(),
        _ => panic!("unknown benchmark: {}", name),
    };
    print!("{}", black_box(value));
}
