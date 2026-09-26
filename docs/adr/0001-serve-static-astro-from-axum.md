# Serve the static Astro client from Axum

Astro will build the browser client as static assets that Axum serves alongside the REST API. This keeps the local application on one server process and one origin, while avoiding a separate Astro server runtime.
