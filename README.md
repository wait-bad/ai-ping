# AI Ping (WaitB) · 模型首字测速台

一款基于 **Tauri 2 + Rust + 原生 Web 前端** 打造的极简、极速、高精度的 LLM / API 接口首字时延（TTFT）与可用性批量测速工具。

---

## ✨ 核心特性

- ⚡ **高精度端到端计时**：精准测量从客户端发包、DNS 解析、TCP/TLS 握手到首个可见 Token（首字）返回的完整链路时延，与中转站后台 TTFT 数据高度对齐。
- 🧠 **全模型/推理架构全覆盖**：完美支持标准 OpenAI 格式，深度兼容 DeepSeek-R1、O1、Claude Thinking 等推理模型的思维链输出（`reasoning_content` / `thought` / `tool_calls`）。
- ⏱️ **实时动态毫秒秒表**：测速时卡片展示高帧率毫秒跳动秒表，界面总耗时与单次首字时延双维度透明展示。
- 🔄 **智能重试与退避机制**：支持网络抖动自动重试，重试成功时展示醒目徽章及总耗时；失败时完整回溯多轮错误原因。
- ⚙️ **全局设置与持久化**：支持自定义超时时间（秒）与重试次数配置，自动持久化存储。
- ⎘ **一键复制新建接口**：支持快捷继承 Base URL 与名称，快速配置新 Key 和模型组。
- 📦 **轻量跨平台**：超小内存占用、极速启动，原生支持 Windows 绿色免安装版与 MSI 安装包。

---

## 🛠️ 构建与运行

### 环境准备
- [Node.js](https://nodejs.org/) (v18+)
- [Rust](https://www.rust-lang.org/) (v1.77+)

### 本地开发
```bash
npm install
npm run tauri dev
```

### 生产打包
```bash
npm run build -- --bundles nsis,msi
```

---

## 📄 License
MIT License
