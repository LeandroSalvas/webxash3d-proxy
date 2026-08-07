import {defineConfig} from 'vite';
import {viteStaticCopy} from 'vite-plugin-static-copy';
import path from 'path';

export default defineConfig({
    plugins: [
        viteStaticCopy({
            targets: [
                {
                    // Copy all WASM files from xash3d-fwgs package (path relative to root: 'src')
                    src: '../node_modules/xash3d-fwgs/dist/*.wasm',
                    dest: '.'
                },
                {
                    // Copy CS 1.6 client files from cs16-client package
                    src: '../node_modules/cs16-client/dist/cstrike',
                    dest: '.'
                },
                {
                    // Copy favicon and logo
                    src: 'favicon.png',
                    dest: '.'
                },
                {
                    src: 'logo.png',
                    dest: '.'
                }
            ]
        })
    ],
    build: {
        outDir: '../../dist',
        emptyOutDir: true,
        rollupOptions: {
            input: {
                main: path.resolve(__dirname, 'src/index.html')
            }
        }
    },
    root: 'src',
});
