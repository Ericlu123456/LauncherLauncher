# 发布流程

1. 构建 Windows GUI、CLI 和 Linux 版本。
2. 只打包**压缩文件**：
   - `launcherlauncher-windows-x64.zip`
   - `launcherlauncher-linux-x86_64.tar.gz`
3. Release **只上传这两个压缩包**。不要上传裸二进制、单独的 README、SHA 文件或测试启动器。
4. 校验和写在 Release notes 或压缩包内的说明里。
5. 示例：

```bash
gh release create vX.Y.Z \
  dist/launcherlauncher/launcherlauncher-windows-x64.zip \
  dist/launcherlauncher/launcherlauncher-linux-x86_64.tar.gz \
  --title "LauncherLauncher vX.Y.Z" \
  --notes-file release-notes.md
```

发布前确认：

- `git status` 干净；
- `cargo test`、`cargo fmt --check` 通过；
- 仓库中没有 `launchers/`、未经验证的启动器说明或个人数据；
- 两个压缩包都能解压并启动。
