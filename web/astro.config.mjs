import { defineConfig } from 'astro/config';

const serverPort = process.env.TODO_PORT || '3000';

export default defineConfig({
  vite: {
    server: {
      proxy: {
        '/api': `http://127.0.0.1:${serverPort}`,
      },
    },
  },
});
