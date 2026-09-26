# Bind the local service to loopback

The unauthenticated service binds only to loopback and accepts browser requests from the served application origin at `localhost` or `127.0.0.1` (plus the documented local Astro development origin). This fits the single-person local use case, avoids exposing personal task data to other devices on a network, and blocks cross-site browser requests and DNS-rebinding hostnames from reaching the local API.
