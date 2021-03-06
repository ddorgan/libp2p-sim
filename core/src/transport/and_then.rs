// Copyright 2018 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use futures::prelude::*;
use multiaddr::Multiaddr;
use std::io::Error as IoError;
use transport::{MuxedTransport, Transport};
use upgrade::Endpoint;

/// See the `Transport::and_then` method.
#[inline]
pub fn and_then<T, C>(transport: T, upgrade: C) -> AndThen<T, C> {
    AndThen { transport, upgrade }
}

/// See the `Transport::and_then` method.
#[derive(Debug, Clone)]
pub struct AndThen<T, C> {
    transport: T,
    upgrade: C,
}

impl<T, C, F, O> Transport for AndThen<T, C>
where
    T: Transport + 'static,
    T::Dial: Send,
    T::Listener: Send,
    T::ListenerUpgrade: Send,
    C: FnOnce(T::Output, Endpoint, &Multiaddr) -> F + Clone + Send + 'static,
    F: Future<Item = O, Error = IoError> + Send + 'static,
{
    type Output = O;
    type Listener = Box<Stream<Item = (Self::ListenerUpgrade, Multiaddr), Error = IoError> + Send>;
    type ListenerUpgrade = Box<Future<Item = O, Error = IoError> + Send>;
    type Dial = Box<Future<Item = O, Error = IoError> + Send>;

    #[inline]
    fn listen_on(self, addr: Multiaddr) -> Result<(Self::Listener, Multiaddr), (Self, Multiaddr)> {
        let upgrade = self.upgrade;

        let (listening_stream, new_addr) = match self.transport.listen_on(addr) {
            Ok((l, new_addr)) => (l, new_addr),
            Err((trans, addr)) => {
                let builder = AndThen {
                    transport: trans,
                    upgrade: upgrade,
                };

                return Err((builder, addr));
            }
        };

        // Try to negotiate the protocol.
        // Note that failing to negotiate a protocol will never produce a future with an error.
        // Instead the `stream` will produce `Ok(Err(...))`.
        // `stream` can only produce an `Err` if `listening_stream` produces an `Err`.
        let stream = listening_stream.map(move |(connection, client_addr)| {
            let upgrade = upgrade.clone();
            let addr = client_addr.clone();
            let future = connection.and_then(move |stream| {
                upgrade(stream, Endpoint::Listener, &addr)
            });

            (Box::new(future) as Box<_>, client_addr)
        });

        Ok((Box::new(stream), new_addr))
    }

    #[inline]
    fn dial(self, addr: Multiaddr) -> Result<Self::Dial, (Self, Multiaddr)> {
        let upgrade = self.upgrade;

        let dialed_fut = match self.transport.dial(addr.clone()) {
            Ok(f) => f,
            Err((trans, addr)) => {
                let builder = AndThen {
                    transport: trans,
                    upgrade: upgrade,
                };

                return Err((builder, addr));
            }
        };

        let future = dialed_fut
            // Try to negotiate the protocol.
            .and_then(move |connection| {
                upgrade(connection, Endpoint::Dialer, &addr)
            });

        Ok(Box::new(future))
    }

    #[inline]
    fn nat_traversal(&self, server: &Multiaddr, observed: &Multiaddr) -> Option<Multiaddr> {
        self.transport.nat_traversal(server, observed)
    }
}

impl<T, C, F, O> MuxedTransport for AndThen<T, C>
where
    T: MuxedTransport + 'static,
    T::Dial: Send,
    T::Listener: Send,
    T::ListenerUpgrade: Send,
    T::Incoming: Send,
    T::IncomingUpgrade: Send,
    C: FnOnce(T::Output, Endpoint, &Multiaddr) -> F + Clone + Send + 'static,
    F: Future<Item = O, Error = IoError> + Send + 'static,
{
    type Incoming = Box<Future<Item = (Self::IncomingUpgrade, Multiaddr), Error = IoError> + Send>;
    type IncomingUpgrade = Box<Future<Item = O, Error = IoError> + Send>;

    #[inline]
    fn next_incoming(self) -> Self::Incoming {
        let upgrade = self.upgrade;

        let future = self.transport.next_incoming().map(|(future, client_addr)| {
            // Try to negotiate the protocol.
            let addr = client_addr.clone();
            let future = future.and_then(move |connection| {
                let upgrade = upgrade.clone();
                upgrade(connection, Endpoint::Listener, &addr)
            });

            (Box::new(future) as Box<Future<Item = _, Error = _> + Send>, client_addr)
        });

        Box::new(future) as Box<_>
    }
}
