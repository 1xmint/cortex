// The animations switch: unchecked turns off tile transitions and
// animations. The choice is remembered across reloads.
(function () {
  var key = "animations";
  var box = document.getElementById("animations");

  function saved() {
    try {
      return window.localStorage.getItem(key) !== "off";
    } catch (error) {
      return true;
    }
  }

  function apply(on) {
    document.documentElement.classList.toggle("no-animations", !on);
  }

  box.checked = true;
  apply(box.checked);

  box.addEventListener("change", function () {
    apply(box.checked);
    try {
      window.localStorage.setItem(key, box.checked ? "on" : "off");
    } catch (error) {
      // Without storage the choice lasts until the page closes.
    }
  });
})();
