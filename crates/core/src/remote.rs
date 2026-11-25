//! Devices on the local network that usbmuxd doesn't list: CoreDevice's own remote pairing over
//! `_remotepairing._tcp` (PITFALLS #50, #55).
//!
//! ```text
//! mDNS _remotepairing._tcp -> TCP -> RPPairing pair-verify (our stored pairing)
//!   -> createListener -> TCP to that port -> TLS-PSK -> CDTunnel
//!   -> userspace TCP stack -> RSD handshake -> the same services as over USB
//! ```
//!
//! A pairing is ours alone: Xcode's isn't readable, and usbmuxd's lockdown pair record is a
//! different thing. One is made per device, by [`pair`], and kept under the state directory.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use idevice::{
    remote_pairing::{
        PAIRABLE_HOST_SERVICE_TYPE, PairableHost, PairableHostInfo, PeerDevice, RemotePairingClient, RpPairingFile,
        RpPairingSocket, connect_tls_psk_tunnel_native,
    },
    tcp::{adapter::Adapter, handle::AdapterHandle},
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use crate::step;

/// The service a device on the network answers pairing and tunnel requests on.
const SERVICE: &str = "_remotepairing._tcp.local.";

/// The name this computer gives itself in pairing. The device shows it in its list of paired
/// computers.
const HOST_NAME: &str = "DeviceSteer";

/// How long [`browse`] listens for every device. Devices that are awake answer the first query
/// within a second; [`find`], which wants one, stops as soon as it answers (0.3 to 1.3 s were
/// seen, PITFALLS #63).
pub(crate) const BROWSE_WAIT: Duration = Duration::from_millis(2500);

/// One device's `_remotepairing._tcp` advert.
#[derive(Debug, Clone)]
pub(crate) struct Advert {
    /// The advert's `identifier`. It changes every few minutes (PITFALLS #55), so it names
    /// nothing for long; [`Advert::matches`] tells whose it is.
    pub identifier: String,
    /// Proof of who the device is for those holding its key: SipHash of `identifier` under the
    /// `altIRK` it gave us when we paired.
    pub auth_tag: String,
    /// Where it answers, IPv4 first.
    pub addrs: Vec<SocketAddr>,
}

impl Advert {
    /// An advert from its TXT record, or `None` when it lacks what a match needs.
    pub(crate) fn from_txt(txt: &HashMap<String, String>, mut addrs: Vec<SocketAddr>) -> Option<Self> {
        let identifier = txt.get("identifier").filter(|s| !s.is_empty())?.clone();
        let auth_tag = txt.get("authTag").filter(|s| !s.is_empty())?.clone();
        // IPv4 first: an IPv6 link-local address needs the right interface scope, which a host
        // with several interfaces (Wi-Fi plus Ethernet on one network) can get wrong.
        addrs.sort_by_key(|a| !a.is_ipv4());
        addrs.dedup();
        Some(Self { identifier, auth_tag, addrs })
    }

    /// Whether this advert is the device `pairing` was made with.
    pub(crate) fn matches(&self, pairing: &Pairing) -> bool {
        pairing
            .file
            .alt_irk()
            .is_some_and(|irk| PeerDevice::validate_auth_tag(irk, &self.identifier, &self.auth_tag))
    }
}

/// Every `_remotepairing._tcp` advert heard within `wait`, one per device.
///
/// Needs no Bonjour service: on Windows the system's own mDNS responder answers (PITFALLS #68),
/// elsewhere our own mDNS socket. A device that sleeps is answered for by a sleep proxy, which macOS's Bonjour caches but a fresh
/// query here may not hear; such a device shows up once it wakes (PITFALLS #55).
pub(crate) async fn browse(wait: Duration) -> Result<Vec<Advert>> {
    browse_until(wait, |_| false).await
}

/// The advert of the device `pairing` was made with, as soon as it is heard with an IPv4 address
/// (within `wait`; then with whatever addresses came).
async fn find(wait: Duration, pairing: &Pairing) -> Result<Option<Advert>> {
    // IPv4 because an IPv6 link-local address alone can fail to connect (see `Advert::from_txt`).
    let adverts = browse_until(wait, |a| a.matches(pairing) && a.addrs.iter().any(SocketAddr::is_ipv4)).await?;
    Ok(adverts.into_iter().find(|a| a.matches(pairing)))
}

/// [`browse`], stopping early at the first advert `done` accepts.
async fn browse_until(wait: Duration, done: impl Fn(&Advert) -> bool) -> Result<Vec<Advert>> {
    let mut answers = Answers::start()?;
    let mut out: Vec<Advert> = Vec::new();
    let end = tokio::time::Instant::now() + wait;
    while let Ok(Some((txt, addrs))) = tokio::time::timeout_at(end, answers.next()).await {
        if let Some(a) = Advert::from_txt(&txt, addrs) {
            // One device answers on every interface it shares with us, under one identifier.
            match out.iter_mut().find(|o| o.identifier == a.identifier) {
                Some(o) => {
                    o.addrs.extend(a.addrs);
                    o.addrs.sort_by_key(|a| !a.is_ipv4());
                    o.addrs.dedup();
                }
                None => out.push(a),
            }
            if out.iter().any(&done) {
                break;
            }
        }
    }
    answers.stop();
    Ok(out)
}

/// `_remotepairing._tcp` instances as they are resolved: TXT record and addresses.
///
/// Windows asks its own mDNS responder: an mDNS socket of ours there makes the firewall ask the
/// person to allow the program (PITFALLS #68). Elsewhere `mdns-sd` sends the queries itself.
#[cfg(windows)]
struct Answers(crate::dnssd::Browse);

#[cfg(windows)]
impl Answers {
    fn start() -> Result<Self> {
        Ok(Self(crate::dnssd::Browse::start(SERVICE)?))
    }

    async fn next(&mut self) -> Option<(HashMap<String, String>, Vec<SocketAddr>)> {
        self.0.next().await.map(|r| (r.txt, r.addrs))
    }

    fn stop(self) {}
}

#[cfg(not(windows))]
struct Answers {
    mdns: mdns_sd::ServiceDaemon,
    rx: mdns_sd::Receiver<mdns_sd::ServiceEvent>,
}

#[cfg(not(windows))]
impl Answers {
    fn start() -> Result<Self> {
        let mdns = mdns_sd::ServiceDaemon::new().context("could not start mDNS discovery")?;
        let rx = mdns.browse(SERVICE).context("could not start mDNS discovery")?;
        Ok(Self { mdns, rx })
    }

    async fn next(&mut self) -> Option<(HashMap<String, String>, Vec<SocketAddr>)> {
        loop {
            let mdns_sd::ServiceEvent::ServiceResolved(info) = self.rx.recv_async().await.ok()? else { continue };
            let txt = info.get_properties().iter().map(|p| (p.key().to_string(), p.val_str().to_string())).collect();
            let port = info.get_port();
            let addrs = info.get_addresses().iter().filter_map(|a| socket_addr(a, port)).collect();
            return Some((txt, addrs));
        }
    }

    fn stop(self) {
        let _ = self.mdns.shutdown();
    }
}

#[cfg(not(windows))]
fn socket_addr(a: &mdns_sd::ScopedIp, port: u16) -> Option<SocketAddr> {
    match a {
        mdns_sd::ScopedIp::V4(v4) => Some(SocketAddr::new(IpAddr::V4(*v4.addr()), port)),
        mdns_sd::ScopedIp::V6(v6) => Some(SocketAddr::V6(std::net::SocketAddrV6::new(*v6.addr(), port, 0, v6.scope_id().index))),
        _ => None,
    }
}

// ---------- the pairing store ----------

/// A pairing we made with one device, and what we last learned about it.
#[derive(Clone)]
pub(crate) struct Pairing {
    pub udid: String,
    pub file: RpPairingFile,
    pub about: About,
}

/// What the device told us, kept so a Wi-Fi device can be listed without opening a tunnel.
/// Personal data (the name): stays in the state directory, never logged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct About {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_version: Option<String>,
}

fn store_dir() -> PathBuf {
    let d = crate::paths::state_dir().join("remote-pairing");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Whether `udid` is safe as a file name: UDIDs are hex digits and dashes.
fn valid_udid(udid: &str) -> bool {
    !udid.is_empty() && udid.len() <= 64 && udid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Every stored pairing. Unreadable files are skipped (and stay on disk).
pub(crate) fn pairings() -> Vec<Pairing> {
    pairings_in(&store_dir())
}

fn pairings_in(dir: &std::path::Path) -> Vec<Pairing> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Pairing> = entries
        .filter_map(|e| {
            let path = e.ok()?.path();
            if path.extension()? != "plist" {
                return None;
            }
            let udid = path.file_stem()?.to_str()?.to_string();
            let file = RpPairingFile::from_bytes(&std::fs::read(&path).ok()?).ok()?;
            let about = std::fs::read(path.with_extension("json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            Some(Pairing { udid, file, about })
        })
        .collect();
    out.sort_by(|a, b| a.udid.cmp(&b.udid));
    out
}

/// The stored pairing for `udid`.
pub(crate) fn pairing(udid: &str) -> Option<Pairing> {
    pairings().into_iter().find(|p| p.udid == udid)
}

fn save_in(dir: &std::path::Path, udid: &str, file: &RpPairingFile, about: &About) -> Result<()> {
    if !valid_udid(udid) {
        bail!("the device sent an unusable UDID; pairing not saved");
    }
    let path = dir.join(format!("{udid}.plist"));
    // Holds this computer's private key for the device: owner-only.
    write_private(&path, &file.to_bytes())?;
    std::fs::write(path.with_extension("json"), serde_json::to_vec_pretty(about)?)?;
    Ok(())
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.write_all(bytes)?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Updates what is known about `udid` after a connection, keeping what this one didn't say.
pub(crate) fn remember(udid: &str, fresh: About) {
    let Some(mut p) = pairing(udid) else { return };
    let a = &mut p.about;
    a.name = fresh.name.or(a.name.take());
    a.product_type = fresh.product_type.or(a.product_type.take());
    a.device_class = fresh.device_class.or(a.device_class.take());
    a.os_version = fresh.os_version.or(a.os_version.take());
    a.build_version = fresh.build_version.or(a.build_version.take());
    let _ = save_in(&store_dir(), udid, &p.file, &p.about);
}

/// Forgets the pairing with `udid`. The device keeps its side until the user removes it there.
pub(crate) fn forget(udid: &str) -> Result<bool> {
    if !valid_udid(udid) {
        bail!("not a UDID: {udid}");
    }
    let path = store_dir().join(format!("{udid}.plist"));
    let existed = path.exists();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("json"));
    Ok(existed)
}

/// The stored pairing each advert belongs to, as (advert, pairing). Adverts that match none are
/// returned apart.
pub(crate) fn match_adverts(adverts: Vec<Advert>, pairings: &[Pairing]) -> (Vec<(Advert, Pairing)>, Vec<Advert>) {
    let mut matched = Vec::new();
    let mut unmatched = Vec::new();
    for a in adverts {
        match pairings.iter().find(|p| a.matches(p)) {
            Some(p) => matched.push((a, p.clone())),
            None => unmatched.push(a),
        }
    }
    (matched, unmatched)
}

// ---------- the tunnel ----------

/// An open tunnel to a device over the network.
pub(crate) struct RemoteTunnel {
    pub adapter: AdapterHandle,
    pub rsd_port: u16,
    /// The pairing connection the tunnel was asked for on. The tunnel was seen to outlive it
    /// (PITFALLS #55); it is kept anyway for the tunnel's life, as nothing says the device won't
    /// tear down tunnels whose pairing connection is gone.
    _control: RemotePairingClient<RpPairingSocket<TcpStream>>,
}

/// Opens a tunnel to `udid` over the network with its stored pairing.
pub(crate) async fn open(udid: &str) -> Result<RemoteTunnel> {
    let mut pairing = pairing(udid).with_context(|| {
        format!("device {udid} is not connected (no usbmuxd connection and no Wi-Fi pairing). Run `devices` to see what is")
    })?;
    let Some(advert) = find(BROWSE_WAIT, &pairing).await? else {
        bail!(
            "device {udid} is not on the network (no USB connection, and no Wi-Fi answer from it). Ask the user to \
             wake and unlock it on the same network as this computer, then retry"
        );
    };

    let stream = connect_any(&advert.addrs, None).await?;
    let mut control = RemotePairingClient::new(RpPairingSocket::new(stream), HOST_NAME);
    step("pairing handshake", control.attempt_pair_verify()).await?;
    match tokio::time::timeout(crate::STEP_TIMEOUT, control.validate_pairing(&mut pairing.file)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => bail!(
            "the device no longer accepts this computer's Wi-Fi pairing ({e:?}); it was removed on the device. \
             Run `devicesteer setup forget --udid {udid}`, then `devicesteer setup wifi` to pair again"
        ),
        Err(_) => bail!("pair-verify timed out (connection stalled)"),
    }
    let port = step("tunnel listener request", control.create_tcp_listener()).await?;
    let stream = connect_any(&advert.addrs, Some(port)).await?;
    let tunnel = step("tunnel TLS handshake", connect_tls_psk_tunnel_native(stream, control.encryption_key())).await?;
    let client: IpAddr = tunnel.info.client_address.parse().context("tunnel setup failed: bad client address")?;
    let server: IpAddr = tunnel.info.server_address.parse().context("tunnel setup failed: bad server address")?;
    let rsd_port = tunnel.info.server_rsd_port;
    let adapter = Adapter::new(Box::new(tunnel.into_inner()), client, server).to_async_handle();
    Ok(RemoteTunnel { adapter, rsd_port, _control: control })
}

/// A TCP connection to the first address that answers (`port` replaces the advert's).
async fn connect_any(addrs: &[SocketAddr], port: Option<u16>) -> Result<TcpStream> {
    let mut last = None;
    for a in addrs {
        let mut a = *a;
        if let Some(p) = port {
            a.set_port(p);
        }
        match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = Some(e.to_string()),
            Err(_) => last = Some("timed out".into()),
        }
    }
    bail!("Wi-Fi connection to the device failed ({})", last.unwrap_or_else(|| "it has no address".into()))
}

// ---------- pairing ----------

/// How pairing is going, for the person doing it.
#[derive(Debug, Clone)]
pub(crate) enum PairEvent {
    /// This computer is offered on the network; the user picks it on the device.
    Advertising,
    /// A device connected and asks for the code; show it to the user, who types it on the device.
    Code(String),
}

/// Pairs with a device on the network, started from the device (iOS 27 and later; PITFALLS #56):
/// this computer offers itself as "DeviceSteer", the user picks it on the device and types the
/// code `events` receives. Returns the new device's UDID once the pairing is saved.
///
/// The device connects to us, so this listens on every interface: the one place DeviceSteer
/// accepts connections from the network. On Windows that makes the firewall ask, once per
/// executable, whether to allow it; a per-user install can't add the rule beforehand (PITFALLS #68).
pub(crate) async fn pair(wait: Duration, events: impl Fn(PairEvent) + Send) -> Result<String> {
    // One listener per family on the same port: `::` takes IPv4 too on macOS, but is IPv6-only
    // on Windows, where IPv4 connections were refused (PITFALLS #59).
    let v4 = tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))
        .await
        .context("could not open a port for the device to connect to")?;
    let port = v4.local_addr()?.port();
    let v6 = match tokio::net::TcpListener::bind(std::net::SocketAddrV6::new(std::net::Ipv6Addr::UNSPECIFIED, port, 0, 0))
        .await
    {
        Ok(l) => Some(l),
        Err(e) => {
            tracing::debug!("no IPv6 listener for pairing: {e}");
            None
        }
    };
    let accept = async {
        match &v6 {
            Some(v6) => tokio::select! {
                r = v4.accept() => r,
                r = v6.accept() => r,
            },
            None => v4.accept().await,
        }
    };

    let mut file = RpPairingFile::generate(HOST_NAME);
    // `generate` derives the identifier from the name, which would make every DeviceSteer
    // computer the same one to the device.
    file.identifier = uuid::Uuid::new_v4().to_string().to_uppercase();
    let info = PairableHostInfo::generate(HOST_NAME, host_model());
    let sid = file.identifier.clone();

    let mdns = mdns_sd::ServiceDaemon::new().context("could not start mDNS")?;
    // The service type's name, `remotepairing-pairable-host`, is past mdns-sd's limit of 15
    // (RFC 6335's); with the default the daemon drops the registration and says nothing
    // (PITFALLS #59). 30 is the most it takes.
    mdns.set_service_name_len_max(30).context("could not advertise this computer")?;
    // Loopback addresses in the advert only send the device to itself.
    let _ = mdns.disable_interface(vec![mdns_sd::IfKind::LoopbackV4, mdns_sd::IfKind::LoopbackV6]);
    let txt = info.mdns_txt_records(&sid);
    let props: Vec<(&str, &str)> = txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let host = format!("devicesteer-{}.local.", sid[..8].to_lowercase());
    let service = mdns_sd::ServiceInfo::new(PAIRABLE_HOST_SERVICE_TYPE, &sid, &host, "", port, &props[..])
        .context("could not advertise this computer")?
        .enable_addr_auto();
    mdns.register(service).context("could not advertise this computer")?;
    events(PairEvent::Advertising);

    let result = async {
        let (stream, _) = match tokio::time::timeout(wait, accept).await {
            Ok(r) => r.context("the device's pairing connection failed")?,
            Err(_) => bail!(
                "no device asked to pair within {}s. On the device, pick \"{HOST_NAME}\" where it lists computers \
                 to pair with, then run `devicesteer setup wifi` again{FIREWALL_HINT}",
                wait.as_secs()
            ),
        };
        let mut host = PairableHost::new(RpPairingSocket::new_device(stream), info);
        let peer = tokio::time::timeout(wait, host.accept(&mut file, |code| async { events(PairEvent::Code(code)) }))
            .await
            .map_err(|_| anyhow::anyhow!("nobody entered the code on the device within {}s", wait.as_secs()))?
            .map_err(|e| anyhow::anyhow!("pairing failed: {e:?}. Run `devicesteer setup wifi` again"))?;
        let about = About { name: Some(peer.name.clone()), product_type: Some(peer.model.clone()), ..About::default() };
        save_in(&store_dir(), &peer.remotepairing_udid, &file, &about)?;
        Ok(peer.remotepairing_udid)
    }
    .await;
    if let Ok(rx) = mdns.shutdown() {
        let _ = rx.recv_timeout(Duration::from_secs(1));
    }
    result
}

/// Added to the timeout when no device connected: on Windows a refused firewall prompt blocks the
/// device's connection without a word (PITFALLS #68).
#[cfg(windows)]
const FIREWALL_HINT: &str = ". If Windows asked whether to allow DeviceSteer on the network and it was not allowed, the \
    device can't connect: ask the user to allow it on private networks in Windows Security > Firewall & network \
    protection > Allow an app through firewall";
#[cfg(not(windows))]
const FIREWALL_HINT: &str = "";

/// What the device is told this computer is. iOS shows computers by a Mac model; on other
/// systems the generic Mac identifier stands in.
fn host_model() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(o) = std::process::Command::new("sysctl").args(["-n", "hw.model"]).output() {
        let m = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if m.starts_with("Mac") || m.contains("Book") {
            return m;
        }
    }
    "Mac17,7".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn txt(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn advert_needs_identifier_and_tag() {
        let a: SocketAddr = "[fe80::1%1]:49152".parse().unwrap();
        let b: SocketAddr = "192.0.2.7:49152".parse().unwrap();
        let ad = Advert::from_txt(&txt(&[("identifier", "ID"), ("authTag", "AAAAAAAA"), ("ver", "26")]), vec![a, b])
            .unwrap();
        assert_eq!(ad.addrs, vec![b, a], "IPv4 first");
        assert!(Advert::from_txt(&txt(&[("identifier", "ID")]), vec![]).is_none());
        assert!(Advert::from_txt(&txt(&[("identifier", ""), ("authTag", "x")]), vec![]).is_none());
    }

    fn pairing_with(irk: [u8; 16], udid: &str) -> Pairing {
        let mut file = RpPairingFile::generate("test");
        file.alt_irk = Some(irk.to_vec());
        Pairing { udid: udid.into(), file, about: About::default() }
    }

    #[test]
    fn adverts_match_by_auth_tag() {
        let irk = [7u8; 16];
        let tag = base64::engine::general_purpose::STANDARD
            .encode(idevice::remote_pairing::compute_auth_tag(&irk, "11111111-2222-3333-4444-555555555555"));
        let ours = Advert { identifier: "11111111-2222-3333-4444-555555555555".into(), auth_tag: tag.clone(), addrs: vec![] };
        // Same tag, other identifier (it rotated, or someone copied the tag): no match.
        let moved = Advert { identifier: "99999999-2222-3333-4444-555555555555".into(), auth_tag: tag, addrs: vec![] };
        let pairings = vec![pairing_with([1u8; 16], "A"), pairing_with(irk, "B")];
        let (m, u) = match_adverts(vec![ours, moved], &pairings);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].1.udid, "B");
        assert_eq!(u.len(), 1);
        // A pairing without an altIRK matches nothing.
        let mut none = pairing_with(irk, "C");
        none.file.alt_irk = None;
        assert!(!m[0].0.matches(&none));
    }

    #[test]
    fn store_round_trip() {
        let dir = std::env::temp_dir().join(format!("devicesteer-store-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = pairing_with([3u8; 16], "00008150-000A1B2C3D4E5F60");
        let about = About { product_type: Some("iPad16,3".into()), ..About::default() };
        save_in(&dir, &p.udid, &p.file, &about).unwrap();
        assert!(save_in(&dir, "../x", &p.file, &about).is_err(), "UDIDs become file names");
        std::fs::write(dir.join("junk.plist"), b"not a plist").unwrap();
        let back = pairings_in(&dir);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].udid, p.udid);
        assert_eq!(back[0].file.alt_irk(), p.file.alt_irk());
        assert_eq!(back[0].file.identifier, p.file.identifier);
        assert_eq!(back[0].about.product_type.as_deref(), Some("iPad16,3"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(format!("{}.plist", p.udid))).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
