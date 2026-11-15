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

  // The page ships with the version it was written for; the GitHub API keeps it current after later releases.
  fetch("https://api.github.com/repos/ihou/DiffCode/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
    .then(function (response) { return response.ok ? response.json() : Promise.reject(response.status); })
    .then(function (release) {
      var version = String(release.tag_name || "").replace(/^v/, "");
      if (version) document.querySelectorAll("[data-version]").forEach(function (el) { el.textContent = version; });
      var zip = (release.assets || []).filter(function (asset) { return asset.name === "DiffCode.zip"; })[0];
      if (zip && zip.size) {
        var size = (zip.size / 1048576).toFixed(1) + " MB";
        document.querySelectorAll("[data-size]").forEach(function (el) { el.textContent = size; });
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
