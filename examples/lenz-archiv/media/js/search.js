// attach a click handler to the search link
var btn = document.querySelector('#global-search-form');
btn.addEventListener('submit', function(event) {

  // don't navigate to that page. Stay put.
  event.preventDefault();

  // make search magic happen instead...

}, false);