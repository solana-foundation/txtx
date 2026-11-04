use txtx_addon_network_evm::EvmNetworkAddon;
use txtx_addon_network_svm::SvmNetworkAddon;
use txtx_core::{kit::Addon, std::StdAddon};

mod macros;

#[macro_use]
extern crate txtx_system_kit;

pub mod cli;
pub mod manifest;
pub mod snapshots;
pub mod term_ui;

pub fn get_available_addons() -> Vec<Box<dyn Addon>> {
    vec![
        Box::new(StdAddon::new()),
        Box::new(SvmNetworkAddon::new()),
        Box::new(EvmNetworkAddon::new()),
    ]
}

pub fn get_addon_by_namespace(namespace: &str) -> Option<Box<dyn Addon>> {
    let available_addons = get_available_addons();
    for addon in available_addons.into_iter() {
        if namespace.starts_with(&format!("{}", addon.get_namespace())) {
            return Some(addon);
        }
    }
    None
}

fn main() {
    cli::main();
}
