use std::{cmp, mem};

use netutils::{n16, Ipv4Addr, Checksum, Ipv4Header, Ipv4};
use resource_scheme::Resource;
use syscall;
use syscall::error::*;

/// A IP (internet protocol) resource.
///
/// Each instance represents a connection (~ a IP socket).
pub struct IpResource {
    /// Link to the underlying device (typically, an Ethernet card).
    pub link: usize,

    /// If this connection was opened waiting for a peer (i.e. `ip:/protocol`),
    /// the data received when the peer actually connected. Otherwise, empty.
    /// Emptied during the first call to `read()`.
    pub init_data: Vec<u8>,

    /// The IP address of the host (i.e. this machine).
    pub host_addr: Ipv4Addr,

    /// The IP address of the peer (i.e. the other machine).
    pub peer_addr: Ipv4Addr,

    /// The IP protocol used by this connection. See
    /// http://www.iana.org/assignments/protocol-numbers/protocol-numbers.xhtml
    /// for the list of valid protocols.
    pub proto: u8,

    /// The id of the next packet being sent.
    /// See https://en.wikipedia.org/wiki/IPv4#Identification .
    pub id: u16,
}

impl Resource for IpResource {
    /// Duplicate the connection.
    ///
    /// This duplicates both `self.link` and `self.init_data`.
    ///
    /// # Errors
    ///
    /// Fails if the `link` to the underlying device cannot be
    /// duplicated.
    fn dup(&self) -> Result<Box<Self>> {
        let link = try!(syscall::dup(self.link));
        Ok(Box::new(IpResource {
            link: link,
            init_data: self.init_data.clone(),
            host_addr: self.host_addr,
            peer_addr: self.peer_addr,
            proto: self.proto,
            id: self.id,
        }))
    }

    /// Get the current path, as `ip:peer/protocol`, where `peer`
    /// is the IPv4 address of the peer and `protocol` is the hex-based
    /// number of the IP protocol used.
    ///
    /// Note that the `peer` is specified even if the connection was initially
    /// created as `ip:/protocol`.
    fn path(&self, buf: &mut [u8]) -> Result<usize> {
        let path_string = format!("ip:{}/{:X}", self.peer_addr.to_string(), self.proto);
        let path = path_string.as_bytes();

        for (b, p) in buf.iter_mut().zip(path.iter()) {
            *b = *p;
        }

        Ok(cmp::min(buf.len(), path.len()))
    }

    /// Read data from the device.
    ///
    /// If some data has already been made available during the establishment
    /// of the connection, this data is (entirely) read during the first call
    /// to `read()`, without attempting to actually read from the device. This
    /// can happen only if the connection was waiting for a remote peer to connect, i.e.
    /// with a url `ip:/protocol`, without host.
    ///
    /// # Errors
    ///
    /// Fails if the call to `syscall::read()` fails for this device.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if !self.init_data.is_empty() {
            let mut data: Vec<u8> = Vec::new();
            mem::swap(&mut self.init_data, &mut data);

            for (b, d) in buf.iter_mut().zip(data.iter()) {
                *b = *d;
            }

            return Ok(cmp::min(buf.len(), data.len()));
        }

        let mut bytes = [0; 65536];
        let count = try!(syscall::read(self.link, &mut bytes));

        if let Some(packet) = Ipv4::from_bytes(&bytes[..count]) {
            if packet.header.proto == self.proto &&
               (packet.header.dst.equals(self.host_addr) || packet.header.dst.equals(Ipv4Addr::BROADCAST)) &&
               (packet.header.src.equals(self.peer_addr) || self.peer_addr.equals(Ipv4Addr::BROADCAST)) {
                for (b, d) in buf.iter_mut().zip(packet.data.iter()) {
                    *b = *d;
                }

                return Ok(cmp::min(buf.len(), packet.data.len()));
            }
        }

        Ok(0)
    }

    /// Send data to the peer.
    ///
    /// # Errors
    ///
    /// Fails if the call to `syscall::write()` fails for this device.
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let ip_data = Vec::from(buf);

        self.id += 1;
        let mut ip = Ipv4 {
            header: Ipv4Header {
                ver_hlen: 0x40 | (mem::size_of::<Ipv4Header>() / 4 & 0xF) as u8, // No Options
                services: 0,
                len: n16::new((mem::size_of::<Ipv4Header>() + ip_data.len()) as u16), // No Options
                id: n16::new(self.id),
                flags_fragment: n16::new(0),
                ttl: 128,
                proto: self.proto,
                checksum: Checksum { data: 0 },
                src: self.host_addr,
                dst: self.peer_addr,
            },
            options: Vec::new(),
            data: ip_data,
        };

        unsafe {
            let header_ptr: *const Ipv4Header = &ip.header;
            ip.header.checksum.data =
                Checksum::compile(Checksum::sum(header_ptr as usize, mem::size_of::<Ipv4Header>()) +
                                  Checksum::sum(ip.options.as_ptr() as usize, ip.options.len()));
        }

        match syscall::write(self.link, &ip.to_bytes()) {
            Ok(_) => Ok(buf.len()),
            Err(err) => Err(err),
        }
    }

    fn sync(&mut self) -> Result<usize> {
        syscall::fsync(self.link)
    }
}

impl Drop for IpResource {
    fn drop(&mut self) {
        let _ = syscall::close(self.link);
    }
}
