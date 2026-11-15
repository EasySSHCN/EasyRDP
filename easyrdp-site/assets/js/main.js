/* EasyRDP Official Site — main.js */
(function () {
  "use strict";

  var SITE_VERSION = "v0.1.0-alpha";

  /* ----- 当前导航高亮 ----- */
  var path = (location.pathname.split("/").pop() || "index.html").toLowerCase();
  document.querySelectorAll(".nav-links a").forEach(function (a) {
    var href = (a.getAttribute("href") || "").toLowerCase();
    if (href === path || (path === "" && href === "index.html")) {
      a.classList.add("active");
    }
  });

  /* ----- 移动端汉堡菜单 ----- */
  var toggle = document.querySelector(".nav-toggle");
  var links = document.querySelector(".nav-links");
  if (toggle && links) {
    toggle.addEventListener("click", function () {
      toggle.classList.toggle("open");
      links.classList.toggle("open");
      toggle.setAttribute("aria-expanded", links.classList.contains("open") ? "true" : "false");
    });
    // 点击链接后自动收起
    links.querySelectorAll("a").forEach(function (a) {
      a.addEventListener("click", function () {
        toggle.classList.remove("open");
        links.classList.remove("open");
      });
    });
  }

  /* ----- 版本号占位注入 ----- */
  document.querySelectorAll("[data-version]").forEach(function (el) {
    el.textContent = SITE_VERSION;
  });

  /* ----- 页脚年份 ----- */
  document.querySelectorAll("[data-year]").forEach(function (el) {
    el.textContent = new Date().getFullYear();
  });
})();
