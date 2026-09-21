// 参照 origin：node 官方 http 模块，keep-alive，256 KiB 响应体。
// 用来做吞吐标定时的「不可质疑的对照物」——排除手写 origin 的影响。
'use strict';

const http = require('http');

const PORT = Number(process.env.ORIGIN_PORT || 18080);
const BODY = Buffer.alloc(256 * 1024, 0x78);

const server = http.createServer((req, res) => {
  res.writeHead(200, {
    'Content-Type': 'application/octet-stream',
    'Content-Length': BODY.length,
  });
  res.end(BODY);
});

server.keepAliveTimeout = 60000;
server.listen(PORT, '127.0.0.1', () => {
  console.log(`origin listening on http://127.0.0.1:${PORT}`);
});

process.on('SIGINT', () => process.exit(0));
process.on('SIGTERM', () => process.exit(0));
