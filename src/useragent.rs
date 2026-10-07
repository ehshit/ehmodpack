pub const USER_AGENT: &str = concat!(
    "EhModPack v",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/ehshit/ehmodpack)"
);

pub fn user_agent() -> &'static str {
    USER_AGENT
}