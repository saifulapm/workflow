//! Which store object a request reaches, and which host names the Worker no
//! longer serves. The host decides, since `cf dev` pins every request to the
//! config's first name; this file lets the host's tests try any name.

/// The walk's store object, which the next- names reach.
pub const WALK: &str = "walk";
/// The real store object, which every other name reaches.
pub const MAIN: &str = "main";

/// Whether `host` is one of `retired`, a comma-separated list of names.
pub fn retired(retired: &str, host: &str) -> bool {
    !host.is_empty() && retired.split(',').any(|name| name.trim() == host)
}

/// The store object a request reaches: the one its X-Hub-Store names, else
/// the walk's on a next- name and the real one on any other. The walk's
/// projects never reach a real replica, and nothing of the walk is deleted.
pub fn object(named: Option<&str>, host: &str) -> &'static str {
    match named {
        Some(MAIN) => MAIN,
        Some(WALK) => WALK,
        _ if host.starts_with("next-") => WALK,
        _ => MAIN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retired_names_are_matched_whole_and_trimmed() {
        let list = "next-hub.on-rexa.com, next-mem.on-rexa.com";
        assert!(retired(list, "next-hub.on-rexa.com"));
        assert!(retired(list, "next-mem.on-rexa.com"));
        assert!(!retired(list, "hub.on-rexa.com"));
        assert!(!retired(list, "mem.on-rexa.com"));
        assert!(!retired(list, "x.next-hub.on-rexa.com"));
        assert!(!retired("", "next-hub.on-rexa.com"));
        assert!(!retired("a,,b", ""));
    }

    #[test]
    fn a_next_name_reaches_the_walk_and_any_other_the_real_store() {
        assert_eq!(object(None, "next-hub.on-rexa.com"), WALK);
        assert_eq!(object(None, "next-mem.on-rexa.com"), WALK);
        assert_eq!(object(None, "hub.on-rexa.com"), MAIN);
        assert_eq!(object(None, "mem.on-rexa.com"), MAIN);
    }

    #[test]
    fn the_header_names_the_object_whatever_the_host() {
        assert_eq!(object(Some("main"), "next-hub.on-rexa.com"), MAIN);
        assert_eq!(object(Some("walk"), "hub.on-rexa.com"), WALK);
        assert_eq!(object(Some("other"), "hub.on-rexa.com"), MAIN);
        assert_eq!(object(Some("other"), "next-hub.on-rexa.com"), WALK);
    }
}
