(async function () {
  const health = document.getElementById('health');
  try {
    const res = await fetch('/api/health');
    const json = await res.json();
    health.textContent = `Backend: ${json.status} (${json.ui})`;
  } catch (err) {
    health.textContent = `Backend unavailable: ${err}`;
  }
})();
