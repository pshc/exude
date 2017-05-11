use proto;

error_chain! {
    errors { BrokenComms }
    foreign_links {
        Bincode(proto::bincoded::Error);
    }
}
