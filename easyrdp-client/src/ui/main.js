// EasyRDP 前端入口：调用 Tauri IPC command 并展示结果。

const { invoke } = window.__TAURI__.core;

const pathInput = document.getElementById("rdp-path");
const output = document.getElementById("output");

function show(label, data) {
  output.textContent = `# ${label}\n${JSON.stringify(data, null, 2)}`;
}

document.getElementById("btn-import").addEventListener("click", async () => {
  const path = pathInput.value.trim();
  if (!path) {
    show("错误", { error: "请输入 .rdp 文件路径" });
    return;
  }
  try {
    const profile = await invoke("import_rdp", { path });
    show("导入成功", profile);
  } catch (e) {
    show("导入失败", { error: String(e) });
  }
});

document.getElementById("btn-list").addEventListener("click", async () => {
  try {
    const profiles = await invoke("list_profiles");
    show("已保存配置", profiles);
  } catch (e) {
    show("失败", { error: String(e) });
  }
});
