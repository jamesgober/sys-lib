//! The baseline for the footprint measurement: a program that prints a line.

fn main() {
    println!("{}", std::hint::black_box("baseline"));
}
