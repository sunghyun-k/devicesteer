//! Marketing names for hardware model identifiers (`ProductType`), so people see "iPhone 12 Pro
//! Max" instead of `iPhone13,4`. Lockdown only reports the identifier; this table is what Apple
//! has published for recent iPhones and iPads. An identifier missing from it shows as itself.

/// The marketing name for a `ProductType` such as `iPhone13,4`, when it is in the table.
pub fn marketing_name(product_type: &str) -> Option<&'static str> {
    MODELS.iter().find(|(ids, _)| ids.contains(&product_type)).map(|(_, name)| *name)
}

const MODELS: &[(&[&str], &str)] = &[
    // iPhone
    (&["iPhone10,1", "iPhone10,4"], "iPhone 8"),
    (&["iPhone10,2", "iPhone10,5"], "iPhone 8 Plus"),
    (&["iPhone10,3", "iPhone10,6"], "iPhone X"),
    (&["iPhone11,2"], "iPhone XS"),
    (&["iPhone11,4", "iPhone11,6"], "iPhone XS Max"),
    (&["iPhone11,8"], "iPhone XR"),
    (&["iPhone12,1"], "iPhone 11"),
    (&["iPhone12,3"], "iPhone 11 Pro"),
    (&["iPhone12,5"], "iPhone 11 Pro Max"),
    (&["iPhone12,8"], "iPhone SE (2nd generation)"),
    (&["iPhone13,1"], "iPhone 12 mini"),
    (&["iPhone13,2"], "iPhone 12"),
    (&["iPhone13,3"], "iPhone 12 Pro"),
    (&["iPhone13,4"], "iPhone 12 Pro Max"),
    (&["iPhone14,4"], "iPhone 13 mini"),
    (&["iPhone14,5"], "iPhone 13"),
    (&["iPhone14,2"], "iPhone 13 Pro"),
    (&["iPhone14,3"], "iPhone 13 Pro Max"),
    (&["iPhone14,6"], "iPhone SE (3rd generation)"),
    (&["iPhone14,7"], "iPhone 14"),
    (&["iPhone14,8"], "iPhone 14 Plus"),
    (&["iPhone15,2"], "iPhone 14 Pro"),
    (&["iPhone15,3"], "iPhone 14 Pro Max"),
    (&["iPhone15,4"], "iPhone 15"),
    (&["iPhone15,5"], "iPhone 15 Plus"),
    (&["iPhone16,1"], "iPhone 15 Pro"),
    (&["iPhone16,2"], "iPhone 15 Pro Max"),
    (&["iPhone17,1"], "iPhone 16 Pro"),
    (&["iPhone17,2"], "iPhone 16 Pro Max"),
    (&["iPhone17,3"], "iPhone 16"),
    (&["iPhone17,4"], "iPhone 16 Plus"),
    (&["iPhone17,5"], "iPhone 16e"),
    (&["iPhone18,1"], "iPhone 17 Pro"),
    (&["iPhone18,2"], "iPhone 17 Pro Max"),
    (&["iPhone18,3"], "iPhone 17"),
    (&["iPhone18,4"], "iPhone Air"),
    // iPad
    (&["iPad7,11", "iPad7,12"], "iPad (7th generation)"),
    (&["iPad11,6", "iPad11,7"], "iPad (8th generation)"),
    (&["iPad12,1", "iPad12,2"], "iPad (9th generation)"),
    (&["iPad13,18", "iPad13,19"], "iPad (10th generation)"),
    (&["iPad15,7", "iPad15,8"], "iPad (A16)"),
    (&["iPad11,1", "iPad11,2"], "iPad mini (5th generation)"),
    (&["iPad14,1", "iPad14,2"], "iPad mini (6th generation)"),
    (&["iPad16,1", "iPad16,2"], "iPad mini (A17 Pro)"),
    (&["iPad11,3", "iPad11,4"], "iPad Air (3rd generation)"),
    (&["iPad13,1", "iPad13,2"], "iPad Air (4th generation)"),
    (&["iPad13,16", "iPad13,17"], "iPad Air (5th generation)"),
    (&["iPad14,8", "iPad14,9"], "iPad Air 11-inch (M2)"),
    (&["iPad14,10", "iPad14,11"], "iPad Air 13-inch (M2)"),
    (&["iPad15,3", "iPad15,4"], "iPad Air 11-inch (M3)"),
    (&["iPad15,5", "iPad15,6"], "iPad Air 13-inch (M3)"),
    (&["iPad8,1", "iPad8,2", "iPad8,3", "iPad8,4"], "iPad Pro 11-inch"),
    (&["iPad8,5", "iPad8,6", "iPad8,7", "iPad8,8"], "iPad Pro 12.9-inch (3rd generation)"),
    (&["iPad8,9", "iPad8,10"], "iPad Pro 11-inch (2nd generation)"),
    (&["iPad8,11", "iPad8,12"], "iPad Pro 12.9-inch (4th generation)"),
    (&["iPad13,4", "iPad13,5", "iPad13,6", "iPad13,7"], "iPad Pro 11-inch (3rd generation)"),
    (&["iPad13,8", "iPad13,9", "iPad13,10", "iPad13,11"], "iPad Pro 12.9-inch (5th generation)"),
    (&["iPad14,3", "iPad14,4"], "iPad Pro 11-inch (4th generation)"),
    (&["iPad14,5", "iPad14,6"], "iPad Pro 12.9-inch (6th generation)"),
    (&["iPad16,3", "iPad16,4"], "iPad Pro 11-inch (M4)"),
    (&["iPad16,5", "iPad16,6"], "iPad Pro 13-inch (M4)"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown() {
        assert_eq!(marketing_name("iPhone13,4"), Some("iPhone 12 Pro Max"));
        assert_eq!(marketing_name("iPad14,4"), Some("iPad Pro 11-inch (4th generation)"));
        assert_eq!(marketing_name("iPhone99,1"), None);
    }

    #[test]
    fn identifiers_are_unique() {
        let mut all: Vec<&str> = MODELS.iter().flat_map(|(ids, _)| ids.iter().copied()).collect();
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n, "an identifier is listed twice");
    }
}
