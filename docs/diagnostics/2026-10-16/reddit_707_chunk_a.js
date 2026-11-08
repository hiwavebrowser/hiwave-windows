// Chunk 1 (hydration starts): the server-rendered posts give way to a skeleton until chunk 2 renders the feed.
L('chunk a ran');
document.getElementById('feed').innerHTML = '<div class="skeleton"></div><div class="skeleton"></div>';
