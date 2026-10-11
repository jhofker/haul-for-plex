// Applies the saved theme before first paint; falls back to the OS setting.
(function () {
  var root = document.documentElement;
  function saved() {
    try { return localStorage.getItem("theme"); } catch (e) { return null; }
  }
  function current() {
    var t = root.getAttribute("data-theme");
    if (t) return t;
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  var t = saved();
  if (t === "dark" || t === "light") root.setAttribute("data-theme", t);
  document.addEventListener("click", function (e) {
    var b = e.target.closest && e.target.closest("#theme-toggle");
    if (!b) return;
    var next = current() === "dark" ? "light" : "dark";
    root.setAttribute("data-theme", next);
    try { localStorage.setItem("theme", next); } catch (err) {}
  });
})();
