#![feature(alloc_system, box_syntax, conservative_impl_trait)]

extern crate alloc_system;
extern crate bincode;
extern crate libc;
extern crate serde;
#[macro_use]
extern crate serde_derive;

mod env;

use std::io::{self, Write};
use std::thread;

#[no_mangle]
pub extern fn version() -> u32 {
    0
}

#[no_mangle]
pub extern fn driver(env: *mut env::DriverEnv) {
    let env = unsafe { Box::from_raw(env) };

    let _input = thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut line = String::new();
        loop {
            match env.try_recv::<env::DownResponse>() {
                Ok(None) => (),
                Ok(Some(resp)) => {
                    println!("=== {:?} ===", resp);
                }
                Err(_) => println!("driver: cannot read")
            }

            print!("> ");
            let _res = stdout.flush();
            debug_assert!(_res.is_ok());

            line.clear();
            let line = match stdin.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => &line,
                Err(e) => {
                    println!("{}", e);
                    break
                }
            };

            let line = line.trim();
            if line == "q" {
                break
            }

            if let Ok(n) = line.parse::<u32>() {
                println!("n: {}", n);
                env.send(&env::UpRequest::Ping(n)).unwrap();
            }
        }

        drop(env);
    });

    _input.join().unwrap();
}
