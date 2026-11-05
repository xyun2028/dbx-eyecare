// 首帧就把明暗定下来，免得深色闪一下再变白。
//
// 放在独立文件里、用同步 script 引在 <head>：宿主给插件 iframe 的 CSP 较严，
// 内联 <script> 有可能被拦掉，外部同源脚本是稳的。
//
// DBX 给的是对象 { appearance, tokens }，不是字符串；老宿主才可能给字符串。
// 这里只做「先定一个」，真正的判断和后续切换在 app.js 里。
(function () {
  var theme = window.dbxPlugin && window.dbxPlugin.theme;
  var appearance = typeof theme === "string" ? theme : theme && theme.appearance;
  if (appearance !== "light" && appearance !== "dark") {
    appearance = window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  document.documentElement.dataset.theme = appearance;
})();
