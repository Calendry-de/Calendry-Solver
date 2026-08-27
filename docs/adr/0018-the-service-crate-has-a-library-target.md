# The service crate has a library target

`convert`, `runs` and `service` used to sit behind `mod` declarations in
`main.rs`. No integration test can link a binary, so none of them had a test
surface — and in practice none of the conversion module's 21 rejection paths and
none of the run registry's state machine was tested at all.

`src/lib.rs` exports the modules; `main.rs` is a thin consumer. The interface is
the test surface, and it now exists.

## Consequences

The four locked-frequency tests moved out of a `#[cfg(test)]` module inside
`convert.rs` into `tests/locked_frequency.rs`, and `convert.rs` shed 313 lines.
`build_output` — pure, and previously uncovered — is snapshot-tested with `insta`.
