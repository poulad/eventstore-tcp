use std::io;
use std::net::SocketAddr;

use futures::Future;

use tokio_core::reactor::Handle;
use tokio_core::io::{Framed, Io};
use tokio_core::net::TcpStream;

use tokio_proto::TcpClient;
use tokio_proto::pipeline::{ClientProto, ClientService};

use tokio_service::Service;

use package::Package;
use codec::PackageCodec;

pub struct EventStoreClient {
    inner: ClientService<TcpStream, PackageProto>,
}

impl EventStoreClient {
    pub fn connect(addr: &SocketAddr, handle: &Handle) -> Box<Future<Item = Self, Error = io::Error>> {
        let ret = TcpClient::new(PackageProto)
            .connect(addr, handle)
            .map(|client_service| {
                EventStoreClient { inner: client_service }
            });

        Box::new(ret)
    }
}

impl Service for EventStoreClient {
    type Request = Package;
    type Response = Package;
    type Error = io::Error;
    type Future = Box<Future<Item = Package, Error = io::Error>>;

    fn call(&self, req: Package) -> Self::Future {
        Box::new(self.inner.call(req))
    }
}

/*
/// Simple middleware
struct Heartbeats<T> {
    inner: T,
}

impl<T> Stream for Heartbeats<T>
    where T: Service<Request = Package, Response = Package, Error = io::Error>,
          T::Future: 'static
{
    type Request = Package;
    type Response = Package;
    type Error = io::Error;
    type Future = Box<Future<Item = Package, Error = io::Error>>;

    fn call(&self, req: Package) -> Self::Future {
        if self.credentials.as_ref().is_some() && req.authentication.is_none() {
            req.authentication = self.credentials.clone();
        }

        self.inner.call(req)
    }
}
*/
struct PackageProto;

impl<T: Io + 'static> ClientProto<T> for PackageProto {
    type Request = Package;
    type Response = Package;

    type Transport = Framed<T, PackageCodec>;
    type BindTransport = Result<Self::Transport, io::Error>;

    fn bind_transport(&self, io: T) -> Self::BindTransport {
        Ok(io.framed(PackageCodec))
    }
}