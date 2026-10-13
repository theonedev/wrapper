package io.onedev.wrapper;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.io.PrintWriter;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/** Optional lifecycle API for applications launched by Wrapper. */
public final class WrapperManager {
    private static final AtomicBoolean stopping = new AtomicBoolean();

    private WrapperManager() {
    }

    private static Socket connect() throws Exception {
        int port = Integer.parseInt(System.getenv("WRAPPER_PORT"));
        Socket socket = new Socket();
        socket.connect(new InetSocketAddress(InetAddress.getByName("127.0.0.1"), port), 10000);
        socket.setSoTimeout(10000);
        return socket;
    }

    private static BufferedReader reader(Socket socket) throws Exception {
        return new BufferedReader(new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8));
    }

    private static void send(Socket socket, String command) throws Exception {
        PrintWriter writer = new PrintWriter(new OutputStreamWriter(socket.getOutputStream(), StandardCharsets.UTF_8), true);
        // The control protocol uses LF on every platform, including Windows.
        writer.print(System.getenv("WRAPPER_TOKEN") + " " + command + "\n");
        if (writer.checkError())
            throw new IllegalStateException("Unable to contact Wrapper");
    }

    static void initialize() throws Exception {
        Socket socket = connect();
        send(socket, "JVM");
        BufferedReader reader = reader(socket);
        if (!"OK".equals(reader.readLine()))
            throw new IllegalStateException("Wrapper rejected JVM connection");
        socket.setSoTimeout(0);
        // System.exit and the last non-daemon thread returning also run hooks.
        // Notify without requesting an exit code or changing the restart policy.
        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            try (Socket notification = connect()) {
                send(notification, "SHUTDOWN");
                reader(notification).readLine();
            } catch (Exception ignored) {
                // The supervisor may already be gone; do not prevent JVM exit.
            }
        }, "Wrapper shutdown notification"));
        Thread control = new Thread(() -> {
            try (socket) {
                String command;
                while ((command = reader.readLine()) != null) {
                    if (command.equals("STOP")) {
                        stopAndReturn(0);
                    }
                }
            } catch (Exception e) {
                // Losing the supervisor must never leave an orphan JVM running.
            }
            supervisorLost();
        }, "Wrapper control");
        control.setDaemon(true);
        control.start();
    }

    private static void supervisorLost() {
        long timeout = 30;
        try {
            long configured = Long.parseLong(System.getenv("WRAPPER_SHUTDOWN_TIMEOUT"));
            if (configured > 0)
                timeout = configured;
        } catch (RuntimeException ignored) {
        }
        // Unlimited operator-initiated shutdown remains supported. Once the
        // supervisor is gone, use a finite fallback so hooks cannot orphan us.
        final long deadline = timeout;
        Thread watchdog = new Thread(() -> {
            try {
                TimeUnit.SECONDS.sleep(deadline);
            } catch (InterruptedException ignored) {
            } finally {
                Runtime.getRuntime().halt(1);
            }
        }, "Wrapper parent-loss watchdog");
        watchdog.setDaemon(true);
        watchdog.start();
        // Keep watching even when an earlier stop is already running hooks.
        if (stopping.compareAndSet(false, true))
            System.exit(1);
    }

    public static void restartAndReturn() {
        exit("restart", 0);
    }

    public static void stopAndReturn(int exitCode) {
        exit("EXIT " + exitCode, exitCode);
    }

    static void failAndReturn() {
        exit("FAIL", 1);
    }

    private static void exit(String command, int exitCode) {
        if (stopping.compareAndSet(false, true)) {
            // System.exit runs shutdown hooks, which can wait for the caller's thread.
            new Thread(() -> {
                try (Socket socket = connect()) {
                    send(socket, command);
                    reader(socket).readLine();
                } catch (Exception e) {
                    System.err.println("Wrapper connection lost: " + e.getMessage());
                } finally {
                    System.exit(exitCode);
                }
            }, "Wrapper shutdown").start();
        }
    }
}
