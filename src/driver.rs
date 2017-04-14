#![feature(alloc_system, box_syntax, conservative_impl_trait)]

extern crate alloc_system;
extern crate bincode;
extern crate serde;

mod env;

use std::io::{self, Write};
use std::thread;

#[no_mangle]
pub extern fn version() -> u32 {
    0
}

#[no_mangle]
pub extern fn driver(env: Box<env::DriverEnv>) {
    let _input = thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut line = String::new();
        loop {
            print!("> ");
            let result = stdout.flush();
            debug_assert!(result.is_ok());

            line.clear();
            let line = match stdin.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => &line,
                Err(e) => {
                    println!("{}", e);
                    break
                }
            };

            if let Ok(n) = line.trim().parse::<u32>() {
                println!("n: {}", n);
                env.tx.send(Some(vec![42; n as usize])).unwrap();
            }
        }
    });
}
