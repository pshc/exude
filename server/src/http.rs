use std::net::SocketAddr;

use hyper::{self, Method, StatusCode};
use hyper::header::ContentLength;
use hyper::server::{Request, Response, Service};
use futures::{Future, Stream, future};
use tokio_core::net::TcpListener;
use tokio_core::reactor::Handle;

use super::{CurrentDriver, DriverInfo};

pub struct DriverService(pub CurrentDriver);

impl Service for DriverService {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = future::FutureResult<Self::Response, Self::Error>;

    fn call(&self, req: Request) -> Self::Future {
        let not_found = || future::ok(Response::new().with_status(StatusCode::NotFound));
        if req.method() != &Method::Get {
            println!("404: {} {}", req.method(), req.path());
            return not_found();
        }
        match *self.0.borrow() {
            Some(ref file) => {
                if &req.path().as_bytes()[1..] == &file.info.digest.hex_bytes()[..] {
                    let bytes = file.bytes.clone();
                    future::ok(
                        Response::new()
                            .with_header(ContentLength(bytes.len() as u64))
                            .with_body(bytes)
                    )
                } else {
                    println!("404: GET {}", req.path());
                    not_found()
                }
            }
            None => {
                println!("404: no driver available");
                not_found()
            }
        }
    }
}

pub fn driver_url(info: &DriverInfo) -> String {
    format!("http://localhost:2003/{}", info.digest)
}

pub fn serve(handle: Handle, current_driver: CurrentDriver) {
    let addr: SocketAddr = ([127, 0, 0, 1], 2003).into();
    let listener = TcpListener::bind(&addr, &handle).expect("http");
    let h = hyper::server::Http::new();
    let handle2 = handle.clone();
    let server = listener
        .incoming()
        .for_each(
            move |(sock, addr)| {
                let service = DriverService(current_driver.clone());
                h.bind_connection(&handle, sock, addr, service);
                Ok(())
            }
        )
        .map_err(|e| println!("http: {}", e));
    handle2.spawn(server);
    println!("Webserver listening on: {}", addr);
}
