//! MQTT client

use crate::APP_TIMEOUT;
use crate::config::Config;
use core::fmt::Display;
use core::net::SocketAddr;
use core::ops::Deref;
use embassy_net::Stack;
use embassy_net::tcp::client::{TcpClient, TcpClientState, TcpConnection};
use embedded_io_async::{Read, Write};
use embedded_nal_async::TcpConnect;
use mqtt_tiny::error::Decoding;
use mqtt_tiny::packets::TryFromIterator;
use mqtt_tiny::{Connack, Connect, Disconnect, Publish};

/// Default TCP and MQTT buffer size
const BUF_SIZE: usize = 1024;

/// A buffer to serialize values in contigous memory
#[derive(Debug, Clone, Copy)]
pub struct MqttBuffer {
    /// The underlying buffer
    buf: [u8; BUF_SIZE],
    /// The buffer length
    len: usize,
}
impl MqttBuffer {
    /// Creates a new, empty MQTT buffer
    pub const fn new() -> Self {
        Self { buf: [0; BUF_SIZE], len: 0 }
    }

    /// Creates a new buffer from the given value by formatting it as string
    pub fn from_display<T>(value: T) -> Self
    where
        T: Display,
    {
        use core::fmt::Write;

        // Allocate self and format value
        let mut this = Self { buf: [0; BUF_SIZE], len: 0 };
        write!(&mut this, "{value}").expect("display value is too large");
        this
    }
}
impl AsRef<[u8]> for MqttBuffer {
    fn as_ref(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}
impl Deref for MqttBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.buf[..self.len]
    }
}
impl core::fmt::Write for MqttBuffer {
    fn write_str(&mut self, str_: &str) -> core::fmt::Result {
        // Allocate target slice
        let remaining = &mut self.buf[self.len..];
        let target = remaining.get_mut(..str_.len()).ok_or(core::fmt::Error)?;

        // Copy data and increment length
        target.copy_from_slice(str_.as_bytes());
        self.len += str_.len();
        Ok(())
    }
}
impl FromIterator<u8> for MqttBuffer {
    fn from_iter<Bytes>(bytes: Bytes) -> Self
    where
        Bytes: IntoIterator<Item = u8>,
    {
        // Collect bytes
        let mut this = Self { buf: [0; BUF_SIZE], len: 0 };
        for byte in bytes {
            // Collect bytes into ad-hoc buffer since we need a slice
            let slot = this.buf.get_mut(this.len).expect("source iterator is too large");
            *slot = byte;
            this.len += 1;
        }
        this
    }
}

/// MQTT stack
pub struct MqttStack {
    /// The associated network stack
    network: Stack<'static>,
    /// The TCP connection state
    tcp_state: TcpClientState<1, BUF_SIZE, BUF_SIZE>,
}
impl MqttStack {
    /// Creates a new MQTT handle and associated state
    pub const fn new(network: Stack<'static>) -> Self {
        // Create state and init self
        let tcp_state = TcpClientState::new();
        Self { network, tcp_state }
    }

    /// Creates an MQTT client, but does not connect yet
    pub fn init(&mut self, config: &Config) -> MqttClient<'_> {
        // Create the TCP client and try to parse the MQTT address
        let tcp_client = TcpClient::new(self.network, &self.tcp_state);
        let address: SocketAddr = config.MQTT_ADDR.parse().expect("invalid mqtt server address");
        MqttClient { tcp_client, address, config: *config }
    }
}

/// An [`MQTT`] client
pub struct MqttClient<'a> {
    /// The TCP clieny connection pool
    tcp_client: TcpClient<'a, 1, BUF_SIZE, BUF_SIZE>,
    /// MQTT server address
    address: SocketAddr,
    /// [`Config`]
    config: Config,
}
impl<'a> MqttClient<'a> {
    /// Connects to the MQTT server
    pub async fn connect(&'a self) -> MqttTcpConnection<'a> {
        // Connect to the MQTT server
        let connection = self.tcp_client.connect(self.address).await.expect("failed to connect to mqtt server");
        MqttTcpConnection { config: self.config, tcp: connection, buf: [0; BUF_SIZE], buf_len: 0 }
    }
}

/// A buffered, iterator-compatible TCP connection adapter
pub struct MqttTcpConnection<'a> {
    config: Config,
    /// The underlying TCP connection
    tcp: TcpConnection<'a, 1, BUF_SIZE, BUF_SIZE>,
    /// A buffer to hold read data
    buf: [u8; BUF_SIZE],
    /// The buffer length
    buf_len: usize,
}
impl<'a> MqttTcpConnection<'a> {
    /// Attempts to login to establish a MQTT application-layer session
    pub async fn login(mut self) -> MqttSession<'a> {
        // Build MQTT connect packet
        let mut connect = Connect::new(APP_TIMEOUT.as_secs() as u16, true, self.config.MQTT_PRFX)
            .expect("failed to assemble mqtt connect packet");
        if self.config.MQTT_USER.len() + self.config.MQTT_PASS.len() > 0 {
            // Set username and password if configured
            connect = (connect.with_username_password(self.config.MQTT_USER, self.config.MQTT_PASS))
                .expect("failed to assemble mqtt connect packet");
        }

        // Send connect packet and await/validate connack packet
        self.send(connect).await;
        let connack = self.recv::<Connack>().await;
        match connack.return_code() {
            0 => MqttSession { connection: self },
            _ => panic!("failed to login to mqtt server"),
        }
    }

    /// Sends an MQTT packet
    async fn send<Packet>(&mut self, packet: Packet)
    where
        Packet: IntoIterator<Item = u8>,
    {
        // Serialize and send the given packet
        let packet: MqttBuffer = packet.into_iter().collect();
        self.tcp.write_all(&packet).await.expect("failed to write mqtt packet");
        self.tcp.flush().await.expect("failed to write mqtt packet");
    }

    /// Receives an MQTT packet
    async fn recv<Packet>(&mut self) -> Packet
    where
        Packet: TryFromIterator,
    {
        // Read packet
        'read_packet: loop {
            // Read some more data
            self.buf_len += self.tcp.read(&mut self.buf[self.buf_len..]).await.expect("failed to read mqtt data");

            // Create a counting iterator over the available bytes
            let mut buf_pos = 0;
            let available = self.buf.iter().take(self.buf_len).inspect(|_| buf_pos += 1).copied();

            // Try to parse the available data
            match Packet::try_from_iter(available) {
                Ok(packet) => {
                    // Consume bytes
                    self.buf.rotate_left(buf_pos);
                    self.buf_len -= buf_pos;
                    break 'read_packet packet;
                }
                Err(e) => match e.variant {
                    Decoding::Truncated => continue 'read_packet,
                    Decoding::SpecViolation => panic!("invalid mqtt packet: {e}"),
                    Decoding::Memory => panic!("mqtt packet is too large: {e}"),
                },
            }
        }
    }
}

/// An established MQTT connection
pub struct MqttSession<'a> {
    /// The MQTT connection
    connection: MqttTcpConnection<'a>,
}
impl MqttSession<'_> {
    /// Publishes an MQTT message
    pub async fn publish(&mut self, topic: &str, payload: &[u8]) {
        use core::fmt::Write;

        // Build topic prefix and suffix parts
        let prefix = self.connection.config.MQTT_PRFX.trim_end_matches('/');
        let suffix = topic.trim_start_matches('/');

        // Assemble final topic
        let mut topic = MqttBuffer::new();
        write!(&mut topic, "{}/{}", prefix, suffix).expect("mqtt topic is too large");

        // Publish message
        // Note: QoS 0 does not expect a puback message
        let publish = Publish::new(&topic, payload, false).expect("failed to assemble mqtt publish packet");
        self.connection.send(publish).await;
    }

    /// Terminates the MQTT session
    pub async fn disconnect(mut self) {
        // Send a disconnect packet to terminate the MQTT session
        let disconnect = Disconnect::new();
        self.connection.send(disconnect).await;
    }
}
