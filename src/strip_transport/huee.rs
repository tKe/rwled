use smart_leds::RGB8;

use std::sync::Arc;
use tokio::net::UdpSocket;
use webrtc_dtls::cipher_suite::CipherSuiteId;
use webrtc_dtls::Error;
use webrtc_dtls::{config::*, conn::DTLSConn};
use webrtc_util::conn::Conn;

#[allow(dead_code)]
pub struct Hue {
    pub(crate) desc: String,
    hub_ip: String,
    username: String,
    clientkey: String,
    group: u16,
    lights: Vec<u16>,
    dtls_conn: Option<Arc<dyn Conn + Send + Sync>>,
    buf: [u8; 106],
}

impl Drop for Hue {
    fn drop(&mut self) {
        let group_url = format!(
            "https://{}/api/{}/groups/{}",
            self.hub_ip, self.username, self.group
        );
        let desc = self.desc.clone();
        tokio::spawn(async move {
            println!("disconnecting from {}", desc);
            reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .unwrap()
                .put(&group_url)
                .body("{\"stream\":{\"active\":false}}")
                .send()
                .await
                .ok()
        });
    }
}

impl Hue {
    pub(crate) async fn new(
        hub_ip: &str,
        username: &str,
        clientkey: &str,
        group: u16,
    ) -> super::Result<Self> {
        let group_url = format!("https://{}/api/{}/groups/{}", hub_ip, username, group);
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();

        client
            .put(&group_url)
            .body("{\"stream\":{\"active\":true}}")
            .send()
            .await?
            .text()
            .await?;

        let grp = client
            .get(&group_url)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let lights: Vec<u16> = Self::get_lights(grp).unwrap();

        let mut hue = Hue {
            desc: format!("{}/{}{:?}", hub_ip, group, lights),
            hub_ip: String::from(hub_ip),
            username: String::from(username),
            clientkey: String::from(clientkey),
            group,
            lights,
            dtls_conn: None,
            buf: [0; 106],
        };

        hue.buf[..9].copy_from_slice("HueStream".as_bytes());
        hue.buf[9..16].copy_from_slice(&[1 as u8, 0, 0, 0, 0, 0, 0]);
        hue.connect().await?;

        Ok(hue)
    }

    fn get_lights(group: serde_json::Value) -> Option<Vec<u16>> {
        group["lights"]
            .as_array()?
            .iter()
            .filter_map(|light| light.as_str().map(|x| x.parse::<u16>().ok()))
            .collect()
    }

    async fn connect(&mut self) -> Result<(), Error> {
        let conn = Arc::new(UdpSocket::bind("0.0.0.0:0").await?);
        conn.connect(format!("{}:2100", self.hub_ip)).await?;
        println!("connecting {}..", self.hub_ip);

        fn decode_hex(s: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
                .collect()
        }

        let key = Arc::new(decode_hex(&self.clientkey).unwrap());

        let config = Config {
            psk: Some(Arc::new(move |_hint: &[u8]| Ok(key.as_ref().clone()))),
            psk_identity_hint: Some(self.username.as_bytes().to_vec()),
            cipher_suites: vec![CipherSuiteId::Tls_Psk_With_Aes_128_Gcm_Sha256],
            extended_master_secret: ExtendedMasterSecretType::Require,
            ..Default::default()
        };
        let dtls_conn: Arc<dyn Conn + Send + Sync> =
            Arc::new(DTLSConn::new(conn, config, true, None).await?);

        println!("connected!");
        self.dtls_conn = Some(dtls_conn);
        Ok(())
    }
}

#[async_trait::async_trait]
impl super::AsyncSmartLedsWrite for Hue {
    type Error = crate::Error;
    type Color = RGB8;

    async fn write<T, I>(&mut self, iterator: T) -> crate::Result<()>
    where
        T: Iterator<Item = I> + Send,
        I: Into<Self::Color>,
    {
        let mut len = 16;
        self.buf[16..]
            .iter_mut()
            .zip(iterator.zip(self.lights.iter()).flat_map(|(item, id)| {
                len += 9;
                let l = item.into();
                let lid = id.to_be_bytes();
                [0, lid[0], lid[1], l.r, l.r, l.g, l.g, l.b, l.g]
            }))
            .for_each(|(dst, itm)| *dst = itm);
        match self.dtls_conn.as_ref() {
            Some(c) => {
                let con = Arc::clone(c);
                con.as_ref()
                    .send(&self.buf[..len])
                    .await
                    .unwrap_or_else(|e| {
                        println!("WARN: disconnected from hue: {:?}", e);
                        self.dtls_conn = None;
                        0
                    });
            }
            None => (),
        };
        Ok(())
    }
}
