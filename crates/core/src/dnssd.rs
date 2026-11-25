//! DNS-SD browsing through Windows' own mDNS responder (the DNS Client service), so this program
//! opens no socket of its own to find devices (PITFALLS #67).
//!
//! A program that binds UDP 5353 to hear mDNS answers, as `mdns-sd` does, makes Windows Defender
//! Firewall ask the person whether to allow it on the network, at first launch and again for
//! every new path of the executable. The DNS Client service already listens for mDNS under a
//! built-in firewall rule; `DnsServiceBrowse` and `DnsServiceResolve` (Windows 10 1809 and later)
//! ask it on our behalf.

use std::{
    collections::{HashMap, HashSet},
    ffi::c_void,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use windows_sys::Win32::{
    Foundation::{DNS_REQUEST_PENDING, ERROR_CANCELLED},
    NetworkManagement::Dns::{
        DNS_QUERY_REQUEST_VERSION1, DNS_RECORDW, DNS_SERVICE_BROWSE_REQUEST, DNS_SERVICE_BROWSE_REQUEST_0,
        DNS_SERVICE_CANCEL, DNS_SERVICE_INSTANCE, DNS_SERVICE_RESOLVE_REQUEST, DNS_TYPE_PTR, DnsFree,
        DnsFreeRecordList, DnsServiceBrowse, DnsServiceBrowseCancel, DnsServiceFreeInstance, DnsServiceResolve,
    },
};

/// One service instance, resolved: its TXT record and where it answers.
pub(crate) struct Resolved {
    pub txt: HashMap<String, String>,
    pub addrs: Vec<SocketAddr>,
}

/// What the callbacks share. Each request the system holds a callback for owns one `Arc` count
/// (passed as its context), released by the callback that ends the request, so the state outlives
/// a callback that arrives after [`Browse`] is dropped.
struct Shared {
    /// Taken when [`Browse`] is dropped: later results go nowhere.
    tx: Mutex<Option<UnboundedSender<Resolved>>>,
    /// Instance names already being resolved: the browse reports one again on every answer.
    seen: Mutex<HashSet<String>>,
    /// The resolve requests' names, which must stay valid while the requests run.
    names: Mutex<Vec<Box<[u16]>>>,
}

/// A running browse for one service type. Results arrive on [`Browse::next`]; dropping it stops
/// the browse.
pub(crate) struct Browse {
    shared: Arc<Shared>,
    rx: UnboundedReceiver<Resolved>,
    cancel: Box<DNS_SERVICE_CANCEL>,
    _query: Box<[u16]>,
}

// SAFETY: the cancel handle is only passed back to the system, which allows that from any thread.
unsafe impl Send for Browse {}

impl Browse {
    /// Starts browsing `service` (such as `_remotepairing._tcp.local`) on every interface.
    pub(crate) fn start(service: &str) -> Result<Self> {
        let (tx, rx) = unbounded_channel();
        let shared = Arc::new(Shared {
            tx: Mutex::new(Some(tx)),
            seen: Mutex::new(HashSet::new()),
            names: Mutex::new(Vec::new()),
        });
        let query = wide(service.trim_end_matches('.'));
        let mut cancel = Box::new(DNS_SERVICE_CANCEL { reserved: std::ptr::null_mut() });
        let context = Arc::into_raw(shared.clone()) as *mut c_void;
        let request = DNS_SERVICE_BROWSE_REQUEST {
            Version: DNS_QUERY_REQUEST_VERSION1,
            InterfaceIndex: 0,
            QueryName: query.as_ptr(),
            Anonymous: DNS_SERVICE_BROWSE_REQUEST_0 { pBrowseCallback: Some(on_browse) },
            pQueryContext: context,
        };
        // SAFETY: `request` and `query` outlive the call; `cancel` is boxed so its address stays
        // valid for DnsServiceBrowseCancel.
        let status = unsafe { DnsServiceBrowse(&request, &mut *cancel) };
        if status != DNS_REQUEST_PENDING {
            // SAFETY: the request was refused, so no callback will release the count it was given.
            unsafe { drop(Arc::from_raw(context as *const Shared)) };
            bail!("could not start mDNS discovery (DnsServiceBrowse failed with {status})");
        }
        Ok(Self { shared, rx, cancel, _query: query })
    }

    /// The next instance resolved, or `None` once the browse has stopped.
    pub(crate) async fn next(&mut self) -> Option<Resolved> {
        self.rx.recv().await
    }
}

impl Drop for Browse {
    fn drop(&mut self) {
        self.shared.tx.lock().unwrap().take();
        // SAFETY: `cancel` was filled by the DnsServiceBrowse that returned pending. The browse
        // callback then runs once more with ERROR_CANCELLED and releases its count.
        unsafe { DnsServiceBrowseCancel(&*self.cancel) };
    }
}

/// Called with each batch of answers to the browse, and with ERROR_CANCELLED when it ends.
unsafe extern "system" fn on_browse(status: u32, context: *const c_void, records: *const DNS_RECORDW) {
    let shared = context as *const Shared;
    if status == ERROR_CANCELLED {
        // SAFETY: this is the browse's last callback; it releases the count `start` gave it.
        unsafe { drop(Arc::from_raw(shared)) };
    } else if status == 0 {
        // SAFETY: the browse holds its count until the ERROR_CANCELLED callback above.
        let shared = unsafe { &*shared };
        let mut r = records;
        while !r.is_null() {
            // SAFETY: a record list from the system, valid until freed below.
            let rec = unsafe { &*r };
            if rec.wType == DNS_TYPE_PTR && rec.dwTtl > 0 {
                // SAFETY: a PTR record's data is DNS_PTR_DATAW.
                let name = unsafe { from_wide(rec.Data.PTR.pNameHost) };
                if let Some(name) = name {
                    resolve(shared, &name);
                }
            }
            r = rec.pNext;
        }
    }
    if !records.is_null() {
        // SAFETY: the list is ours to free once read.
        unsafe { DnsFree(records as *const c_void, DnsFreeRecordList) };
    }
}

/// Starts resolving one instance the browse reported, once per name.
fn resolve(shared: &Shared, name: &str) {
    if shared.tx.lock().unwrap().is_none() || !shared.seen.lock().unwrap().insert(name.to_string()) {
        return;
    }
    let mut query = wide(name);
    // SAFETY: `shared` is borrowed from a live `Arc` (the browse's count); this adds one for the
    // resolve's callback.
    let context = unsafe {
        let ptr = shared as *const Shared;
        Arc::increment_strong_count(ptr);
        ptr as *mut c_void
    };
    let request = DNS_SERVICE_RESOLVE_REQUEST {
        Version: DNS_QUERY_REQUEST_VERSION1,
        InterfaceIndex: 0,
        QueryName: query.as_mut_ptr(),
        pResolveCompletionCallback: Some(on_resolve),
        pQueryContext: context,
    };
    // Not cancelled: a resolve ends by itself (answered or timed out) and its callback finds no
    // receiver once the browse is gone.
    let mut cancel = DNS_SERVICE_CANCEL { reserved: std::ptr::null_mut() };
    // SAFETY: `request` lives through the call; the name is kept in `shared.names` beyond it.
    let status = unsafe { DnsServiceResolve(&request, &mut cancel) };
    if status == DNS_REQUEST_PENDING {
        shared.names.lock().unwrap().push(query);
    } else {
        // SAFETY: refused, so no callback will release this count.
        unsafe { drop(Arc::from_raw(context as *const Shared)) };
        shared.seen.lock().unwrap().remove(name);
    }
}

/// Called once per resolve, with the instance or an error.
unsafe extern "system" fn on_resolve(status: u32, context: *const c_void, instance: *const DNS_SERVICE_INSTANCE) {
    // SAFETY: the count `resolve` added for this request, released here.
    let shared = unsafe { Arc::from_raw(context as *const Shared) };
    if instance.is_null() {
        return;
    }
    if status == 0 {
        // SAFETY: an instance from the system, valid until freed below.
        let resolved = unsafe { read_instance(&*instance) };
        if let Some(tx) = shared.tx.lock().unwrap().as_ref() {
            let _ = tx.send(resolved);
        }
    }
    // SAFETY: the instance is ours to free.
    unsafe { DnsServiceFreeInstance(instance) };
}

/// # Safety
/// `inst` must be a DNS_SERVICE_INSTANCE the system filled in.
unsafe fn read_instance(inst: &DNS_SERVICE_INSTANCE) -> Resolved {
    let mut txt = HashMap::new();
    for i in 0..inst.dwPropertyCount as usize {
        // SAFETY: `keys` and `values` hold `dwPropertyCount` strings each.
        let (k, v) = unsafe { (from_wide(*inst.keys.add(i)), from_wide(*inst.values.add(i))) };
        if let Some(k) = k {
            txt.insert(k, v.unwrap_or_default());
        }
    }
    let port = inst.wPort;
    let mut addrs = Vec::new();
    if !inst.ip4Address.is_null() {
        // SAFETY: non-null; an IP4_ADDRESS holds the address bytes in network order.
        let v4 = unsafe { *inst.ip4Address };
        addrs.push(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(v4.to_ne_bytes())), port));
    }
    if !inst.ip6Address.is_null() {
        // SAFETY: non-null; read as bytes.
        let v6 = Ipv6Addr::from(unsafe { (*inst.ip6Address).IP6Byte });
        // A link-local address only works through the interface it was heard on.
        let scope = if v6.is_unicast_link_local() { inst.dwInterfaceIndex } else { 0 };
        addrs.push(SocketAddr::V6(SocketAddrV6::new(v6, port, 0, scope)));
    }
    Resolved { txt, addrs }
}

fn wide(s: &str) -> Box<[u16]> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// # Safety
/// `p` is null or a NUL-terminated UTF-16 string.
unsafe fn from_wide(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0;
    // SAFETY: NUL-terminated.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units precede the NUL.
    Some(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) }))
}
