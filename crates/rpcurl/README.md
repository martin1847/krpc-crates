


## rpcurl

krpc客户端测试工具
```bash
cargo add clap --features derive
```

```bash
RPC_TOKEN2=env.token 
export REMOTE=http://127.0.0.1:50051
# 🍀 测试正常返回
cargo run $REMOTE/demo-java-server/Demo/hello -d '{"name":"我是Rust","age":28}' -H a=123 -H 123=c-id -v
# 🍀 文件作为输入数据
cargo run $REMOTE/demo-java-server/Demo/hello -f test.json
# ❌ 测试logicError
cargo run $REMOTE/demo-java-server/Demo/testLogicError -d 123
# 🌱 测试bytes
cargo run $REMOTE/demo-java-server/Demo/bytesTime
```
