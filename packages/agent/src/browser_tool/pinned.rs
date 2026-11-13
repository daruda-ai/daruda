//! The pinned agent-browser release and its expected digests.
//!
//! Upstream publishes no checksum file, so these digests are the trust root.
//! To bump: download every asset of the new release, run `shasum -a 256` on
//! each, then update [`VERSION`] and every digest below together.

use super::Asset;

pub const VERSION: &str = "v0.43.0";

pub(super) const DARWIN_ARM64: Asset = Asset {
    name: "agent-browser-darwin-arm64",
    sha256: "05338a8eeddf0b3f9ebf0c87a6c21aedc10685608c68618e1a8483c43eae9c3f",
};
pub(super) const DARWIN_X64: Asset = Asset {
    name: "agent-browser-darwin-x64",
    sha256: "49a5ab27297213c1bc0005701a6a4542399abfc9d2cb7520ab519ebb94b8b13d",
};
pub(super) const LINUX_ARM64: Asset = Asset {
    name: "agent-browser-linux-arm64",
    sha256: "34be55c2b92d8d199fb9d1d62a905ee5b6ac5b1f3e8c24e5b20597f370d905e5",
};
pub(super) const LINUX_MUSL_ARM64: Asset = Asset {
    name: "agent-browser-linux-musl-arm64",
    sha256: "f1d146ad95c1df627b3055475fcb971dfd02f757a1eef756a8f2bd9e19ffff4a",
};
pub(super) const LINUX_MUSL_X64: Asset = Asset {
    name: "agent-browser-linux-musl-x64",
    sha256: "10b3b1d5fa8f53f7bc44273bcefaa64299ab77071bc83e75d96de5ed66025c89",
};
pub(super) const LINUX_X64: Asset = Asset {
    name: "agent-browser-linux-x64",
    sha256: "8e69c9fb662514c9cf36e4911e94e913742ba066e55102486350e049dd847612",
};
pub(super) const WIN32_X64: Asset = Asset {
    name: "agent-browser-win32-x64.exe",
    sha256: "ebcef5ebea93c7ccf0fe3f8c18eee908ae9174dfef8e52a3dda6f24a59f630dc",
};
