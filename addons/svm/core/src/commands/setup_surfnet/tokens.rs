use solana_pubkey::Pubkey;

const MAINNET_AAVE_PROGRAM_ID: &str = "3vAs4D1WE6Na4tCgt4BApgFfENbm8WY7q4cSPD1yM4Cg";
const MAINNET_AUDIO_PROGRAM_ID: &str = "9LzCMqDgTKYz9Drzqnpgee3SGa89up3a247ypMj2xrqM";
const MAINNET_BONK_PROGRAM_ID: &str = "DezXAZ8z7PnrnRJjz3wXBoRgixCa6xjnB7YaB1pPB263";
const MAINNET_GALA_PROGRAM_ID: &str = "eEUiUs4JWYZrp72djAGF1A8PhpR6rHphGeGN7GbVLp6";
const MAINNET_GRT_PROGRAM_ID: &str = "HGsLG4PnZ28L8A4R5nPqKgZd86zUUdmfnkTRnuFJ5dAX";
const MAINNET_GT_PROGRAM_ID: &str = "ABAq2R9gSpDDGguQxBk4u13s4ZYW6zbwKVBx15mCMG8";
const MAINNET_HNT_PROGRAM_ID: &str = "hntyVP6YFm1Hg25TN9WGLqM12b8TQmcknKrdu1oxWux";
const MAINNET_LDO_PROGRAM_ID: &str = "HZRCwxP2Vq9PCpPXooayhJ2bxTpo5xfpQrwB1svh332p";
const MAINNET_LINK_PROGRAM_ID: &str = "CWE8jPTUYhdCTZYWPTe1o5DFqfdjzWKc9WKz6rSjQUdG";
const MAINNET_JLP_PROGRAM_ID: &str = "27G8MtK7VtTcCHkpASjSDdkWWYfoqT6ggEuKidVJidD4";
const MAINNET_JUP_PROGRAM_ID: &str = "JUPyiwrYJFskUPiHa7hkeR8VUtAeFoSYbKedZNsDvCN";
const MAINNET_PAXG_PROGRAM_ID: &str = "C6oFsE8nXRDThzrMEQ5SxaNFGKoyyfWDDVPw37JKvPTe";
const MAINNET_RENDER_PROGRAM_ID: &str = "rndrizKT3MK1iimdxRdWabcF7Zg7AR5T4nud4EkHBof";
const MAINNET_TRUMP_PROGRAM_ID: &str = "6p6xgHyF7AeE6TZkSmFsko444wqoP15icUSqi2jfGiPN";
const MAINNET_USDC_PROGRAM_ID: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const MAINNET_USDT_PROGRAM_ID: &str = "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB";
const MAINNET_WSOL_PROGRAM_ID: &str = "So11111111111111111111111111111111111111112";

pub fn get_token_by_name(network: &str, name: &str) -> Option<Pubkey> {
    match (network, name.to_ascii_lowercase().as_str()) {
        ("mainnet", "aave") => Some(Pubkey::from_str_const(MAINNET_AAVE_PROGRAM_ID)),
        ("mainnet", "audio") => Some(Pubkey::from_str_const(MAINNET_AUDIO_PROGRAM_ID)),
        ("mainnet", "bonk") => Some(Pubkey::from_str_const(MAINNET_BONK_PROGRAM_ID)),
        ("mainnet", "gala") => Some(Pubkey::from_str_const(MAINNET_GALA_PROGRAM_ID)),
        ("mainnet", "grt") => Some(Pubkey::from_str_const(MAINNET_GRT_PROGRAM_ID)),
        ("mainnet", "gt") => Some(Pubkey::from_str_const(MAINNET_GT_PROGRAM_ID)),
        ("mainnet", "hnt") => Some(Pubkey::from_str_const(MAINNET_HNT_PROGRAM_ID)),
        ("mainnet", "ldo") => Some(Pubkey::from_str_const(MAINNET_LDO_PROGRAM_ID)),
        ("mainnet", "link") => Some(Pubkey::from_str_const(MAINNET_LINK_PROGRAM_ID)),
        ("mainnet", "jlp") => Some(Pubkey::from_str_const(MAINNET_JLP_PROGRAM_ID)),
        ("mainnet", "jup") => Some(Pubkey::from_str_const(MAINNET_JUP_PROGRAM_ID)),
        ("mainnet", "paxg") => Some(Pubkey::from_str_const(MAINNET_PAXG_PROGRAM_ID)),
        ("mainnet", "render") => Some(Pubkey::from_str_const(MAINNET_RENDER_PROGRAM_ID)),
        ("mainnet", "trump") => Some(Pubkey::from_str_const(MAINNET_TRUMP_PROGRAM_ID)),
        ("mainnet", "usdc") => Some(Pubkey::from_str_const(MAINNET_USDC_PROGRAM_ID)),
        ("mainnet", "usdt") => Some(Pubkey::from_str_const(MAINNET_USDT_PROGRAM_ID)),
        ("mainnet", "wsol") => Some(Pubkey::from_str_const(MAINNET_WSOL_PROGRAM_ID)),
        _ => None,
    }
}
