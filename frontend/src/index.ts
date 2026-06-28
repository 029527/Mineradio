// 脚手架占位入口。真实前端将在任务 D 迁入。
const status = document.getElementById('status');

async function probe() {
  if (!status) return;
  try {
    const res = await fetch('/api/app/version');
    if (res.ok) {
      const data = await res.json();
      status.textContent = `后端已连通 · 版本 ${data.version ?? '?'}`;
      return;
    }
    status.textContent = `后端响应异常 (${res.status})`;
  } catch {
    status.textContent = 'Tauri + rsbuild 脚手架就绪（后端未启动）';
  }
}

probe();
