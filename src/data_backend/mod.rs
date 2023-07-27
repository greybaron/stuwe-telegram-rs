
cfg_if! {
    if #[cfg(feature = "mensimates")] {
        pub mod mm_parser;
    } else {
        pub mod stuwe_parser;
    }
}