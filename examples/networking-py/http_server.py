"""Minimal synchronous HTTP server for testing inbound connections."""
import socket

HOST = "0.0.0.0"
PORT = 8080

srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind((HOST, PORT))
srv.listen(1)
print(f"HTTP server listening on {HOST}:{PORT}...")

while True:
    conn, addr = srv.accept()
    print(f"Connection from {addr}")

    request = b""
    while b"\r\n\r\n" not in request:
        chunk = conn.recv(4096)
        if not chunk:
            break
        request += chunk

    first_line = request.split(b"\r\n")[0].decode()
    print(f"Request: {first_line}")

    body = "Hello from Hyperlight/Unikraft!"
    response = (
        f"HTTP/1.1 200 OK\r\n"
        f"Content-Length: {len(body)}\r\n"
        f"Content-Type: text/plain\r\n"
        f"Connection: close\r\n"
        f"\r\n"
        f"{body}"
    )
    conn.sendall(response.encode())
    conn.close()
    print("Response sent, connection closed.")
    break  # exit after one request for testing

srv.close()
print("Server done.")
