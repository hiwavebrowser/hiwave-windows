// Chunk 2 (the feed): an IntersectionObserver on the sentinel renders the posts.
L('chunk b ran');
new IntersectionObserver(function (entries) {
  if (!entries[0].isIntersecting) return;
  var feed = document.getElementById('feed'); feed.innerHTML = '';
  for (var i = 1; i <= 3; i++) { var d = document.createElement('div'); d.className = 'post'; d.textContent = 'Feed post ' + i; feed.appendChild(d); }
  L('IO rendered ' + feed.children.length + ' posts');
}).observe(document.getElementById('sentinel'));
