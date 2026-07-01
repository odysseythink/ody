# ThreadManager Sample

Small one-shot binary that starts a Ody thread with `ThreadManager` from
`ody-core-api`, submits a single user turn, and prints the final assistant
message.

```sh
cargo run -p ody-thread-manager-sample -- "Say hello"
```

Use `--model` to override the configured default model:

```sh
cargo run -p ody-thread-manager-sample -- --model gpt-5.2 "Say hello"
```

The prompt can also be piped through stdin:

```sh
printf 'Say hello\n' | cargo run -p ody-thread-manager-sample
```
