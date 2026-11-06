//! An application-scoped device id, and what this machine's sources look like.
//!
//! ```text
//! cargo run --example device_id --features device-id
//! ```

use sys_lib::device;

fn main() {
    println!("{}", device::report());
    match device::id("com.example.device-id-example") {
        Ok(id) => println!("device id: {id}"),
        Err(e) => println!("no device id: {e}"),
    }
}
