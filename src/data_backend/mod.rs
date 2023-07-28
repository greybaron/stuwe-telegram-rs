
cfg_if! {
    if #[cfg(feature = "mensimates")] {
        pub mod mm_parser;
    } else {
        pub mod stuwe_parser;
    }
}

const EMOJIS: [&str; 7] = ["☀️", "🦀", "💂🏻‍♀️", "☕️", "☝🏻", "🌤️", "🥦"];

fn escape_markdown_v2(input: &str) -> String {
    // all 'special' chars have to be escaped when using telegram markdown_v2

    input
        .replace('.', r"\.")
        .replace('!', r"\!")
        .replace('+', r"\+")
        .replace('-', r"\-")
        .replace('<', r"\<")
        .replace('>', r"\>")
        .replace('(', r"\(")
        .replace(')', r"\)")
        .replace('=', r"\=")
        // workaround as '&' in html is improperly decoded
        .replace("&amp;", "&")
}