//! Built-in compatibility profiles, grouped by launcher.
//!
//! Rules are maintained from local reproduction and testing. They match only
//! whole methods and only methods with a compatible return type; every rule
//! that found no target is reported instead of being guessed.

mod hmcl;
mod pcl2;
mod pcl2ce;

pub use hmcl::hmcl;
pub use pcl2::pcl2;
pub use pcl2ce::pcl2ce;

use super::{Action, Rule};

pub(super) fn rule(
    class: &str,
    method: &str,
    desc: Option<&str>,
    action: Action,
    note: &str,
) -> Rule {
    Rule {
        id: String::new(),
        class: class.to_string(),
        method: method.to_string(),
        desc: desc.map(str::to_string),
        action,
        note: note.to_string(),
    }
}

pub fn for_launcher(label: &str, remove_updates: bool) -> Vec<Rule> {
    match label {
        "HMCL" => hmcl(remove_updates),
        "PCL2-CE" => pcl2ce(remove_updates),
        "PCL2" => pcl2(remove_updates),
        _ => Vec::new(),
    }
}
