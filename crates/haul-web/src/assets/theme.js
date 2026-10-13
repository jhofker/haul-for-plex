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
  // Menus in a filter or search form apply as soon as they change.
  document.addEventListener("change", function (e) {
    var t = e.target;
    if (t.tagName !== "SELECT" || !t.form || !t.form.hasAttribute("data-autosubmit")) return;
    if (t.form.requestSubmit) t.form.requestSubmit(); else t.form.submit();
  });
  // Hide cover images the server couldn't provide (error events don't bubble).
  document.addEventListener("error", function (e) {
    if (e.target && e.target.tagName === "IMG") e.target.style.visibility = "hidden";
  }, true);
})();
