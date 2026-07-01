# ody-protocol

This crate defines the "types" for the protocol used by Ody CLI, which includes both "internal types" for communication between `ody-core` and `ody-tui`, as well as "external types" used with `ody app-server`.

This crate should have minimal dependencies.

Ideally, we should avoid "material business logic" in this crate, as we can always introduce `Ext`-style traits to add functionality to types in other crates.
