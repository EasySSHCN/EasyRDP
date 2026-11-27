$ErrorActionPreference='Continue'
$dir='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\manual'
$footer=@'
<footer class="site-footer">
    <div class="container">
      <div class="footer-grid">
        <div class="footer-brand">
          <a class="logo" href="../index.html">
            <svg width="26" height="26" viewBox="0 0 32 32" fill="none" aria-hidden="true">
              <rect width="32" height="32" rx="7" fill="#3b82f6"/>
              <rect x="5" y="7" width="22" height="14" rx="2" stroke="#fff" stroke-width="2"/>
              <path d="M16 14l6-4M16 14l-4-2" stroke="#fff" stroke-width="2" stroke-linecap="round"/>
              <path d="M12 25h8" stroke="#fff" stroke-width="2" stroke-linecap="round"/>
            </svg>
            EasyRDP
          </a>
          <p>跨平台开源远程桌面客户端。Rust + Tauri 2 + FreeRDP，MIT 协议，让每一次远程连接都安全可控。</p>
        </div>
        <div class="footer-col">
          <h4>产品</h4>
          <a href="../index.html">首页</a>
          <a href="../features.html">功能介绍</a>
          <a href="../download.html">下载</a>
          <a href="../changelog.html">更新日志</a>
          <a href="../docs.html">使用文档</a>
        </div>
        <div class="footer-col">
          <h4>资源</h4>
          <a href="https://github.com/EasySSHCN/EasyRDP" target="_blank" rel="noopener">GitHub 仓库</a>
          <a href="https://gitee.com/easyssh/EasyRDP" target="_blank" rel="noopener">Gitee 镜像</a>
          <a href="../docs.html">JSON Profile 规范</a>
          <a href="../migrate.html">迁移指南</a>
          <a href="../compare.html">产品对比</a>

          <a href="../faq.html">常见问题</a>
          <a href="../security.html">安全策略</a>
        </div>
        <div class="footer-col">
          <h4>社区</h4>
          <a href="../contributing.html">参与贡献</a>
          <a href="https://github.com/EasySSHCN/EasyRDP/issues" target="_blank" rel="noopener">问题反馈</a>
          <a href="https://github.com/EasySSHCN/EasyRDP/discussions" target="_blank" rel="noopener">讨论区</a>
          <a href="../license.html">MIT 协议</a>
        </div>
      </div>
      <div class="footer-bottom">
        <span>© <span data-year>2026</span> EasyRDP Contributors · 基于 MIT 协议开源</span>
        <span>备案号占位：京ICP备XXXXXXXX号</span>
      </div>
    </div>
  </footer>
'@
$scriptTag='<script src="../assets/js/main.js?v=24"></script>'
Get-ChildItem $dir -Filter *.html | ForEach-Object {
  $p=$_.FullName
  $t=[IO.File]::ReadAllText($p,[Text.UTF8Encoding]::new($false))
  $i=$t.IndexOf('<footer class="site-footer">')
  if($i -lt 0){ "NO-FOOTER $($_.Name)"; return }
  $j=$t.IndexOf('</footer>')
  if($j -lt 0){ "NO-CLOSE $($_.Name)"; return }
  $t = $t.Substring(0,$i) + $footer + $t.Substring($j+9)
  # main.js 加缓存版本参数（统一与全站一致）
  $t = $t.Replace('<script src="../assets/js/main.js"></script>',$scriptTag)
  [IO.File]::WriteAllText($p,$t,[Text.UTF8Encoding]::new($false))
  "footer-ok $($_.Name)"
}