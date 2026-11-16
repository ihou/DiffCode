// DiffCode website: remember the language a visitor picks, show the latest release, fade sections in.
(function () {
  "use strict";

  document.querySelectorAll("[data-lang]").forEach(function (link) {
    link.addEventListener("click", function () {
      try { localStorage.setItem("diffcode-lang", link.getAttribute("data-lang")); } catch (e) { /* private mode */ }
    });
  });

  var nav = document.querySelector(".nav");
  if (nav) {
    var onScroll = function () { nav.classList.toggle("scrolled", window.scrollY > 8); };
    window.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }

  // Mac first unless the visitor is on Windows.
  if (/Windows/.test(navigator.userAgent || "")) {
    document.querySelectorAll(".cta").forEach(function (cta) {
      var mac = cta.querySelector('[data-os="mac"]'), win = cta.querySelector('[data-os="windows"]');
      if (!mac || !win) return;
      mac.classList.add("alt");
      win.classList.remove("alt");
      cta.insertBefore(win, mac);
    });
  }

  // The page ships with the version it was written for; the GitHub API keeps it current after later releases.
  fetch("https://api.github.com/repos/ihou/DiffCode/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
    .then(function (response) { return response.ok ? response.json() : Promise.reject(response.status); })
    .then(function (release) {
      var version = String(release.tag_name || "").replace(/^v/, "");
      if (version) document.querySelectorAll("[data-version]").forEach(function (el) { el.textContent = version; });
      var pkg = (release.assets || []).filter(function (asset) { return asset.name === "DiffCode.pkg"; })[0];
      if (pkg && pkg.size) {
        var size = (pkg.size / 1048576).toFixed(1);
        document.querySelectorAll("[data-size]").forEach(function (el) { el.textContent = size; });
        var rounded = String(Math.max(1, Math.round(pkg.size / 1048576)));
        document.querySelectorAll("[data-size-round]").forEach(function (el) { el.textContent = rounded; });
      }
      var exe = (release.assets || []).filter(function (asset) { return asset.name === "DiffCode-Setup-x64.exe"; })[0];
      if (exe && exe.size) {
        var exeSize = (exe.size / 1048576).toFixed(1);
        document.querySelectorAll("[data-size-win]").forEach(function (el) { el.textContent = exeSize; });
      }
    })
    .catch(function () { /* keep the built-in text */ });

  var items = document.querySelectorAll(".reveal");
  if (!("IntersectionObserver" in window)) {
    items.forEach(function (el) { el.classList.add("in"); });
    return;
  }
  var observer = new IntersectionObserver(function (entries) {
    entries.forEach(function (entry) {
      if (entry.isIntersecting) {
        entry.target.classList.add("in");
        observer.unobserve(entry.target);
      }
    });
  }, { rootMargin: "0px 0px -8% 0px", threshold: 0.08 });
  items.forEach(function (el) { observer.observe(el); });
})();
