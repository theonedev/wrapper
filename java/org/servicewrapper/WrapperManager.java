package org.servicewrapper;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.io.PrintWriter;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.atomic.AtomicBoolean;

/** Optional lifecycle API for applications launched by ServiceWrapper. */
public final class WrapperManager {
    private static final AtomicBoolean stopping = new AtomicBoolean();

    private WrapperManager() {
    }

    private static Socket connect() throws Exception {
        int port = Integer.parseInt(System.getenv("SERVICEWRAPPER_PORT"));
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
        writer.println(System.getenv("SERVICEWRAPPER_TOKEN") + " " + command);
        if (writer.checkError())
            throw new IllegalStateException("Unable to contact ServiceWrapper");
    }

    static void initialize() throws Exception {
        Socket socket = connect();
        send(socket, "JVM");
        BufferedReader reader = reader(socket);
        if (!"OK".equals(reader.readLine()))
            throw new IllegalStateException("ServiceWrapper rejected JVM connection");
        socket.setSoTimeout(0);
        Thread control = new Thread(() -> {
            try (socket) {
                String command;
                while ((command = reader.readLine()) != null) {
                    if (command.equals("STOP")) {
                        stopAndReturn(0);
                        return;
                    }
                }
            } catch (Exception e) {
                // Losing the supervisor must never leave an orphan JVM running.
            }
            stopAndReturn(1);
        }, "ServiceWrapper control");
        control.setDaemon(true);
        control.start();
    }

    public static void restartAndReturn() {
        exit("restart", 0);
    }

    public static void stopAndReturn(int exitCode) {
        exit("EXIT " + exitCode, exitCode);
    }

    private static void exit(String command, int exitCode) {
        if (stopping.compareAndSet(false, true)) {
            // System.exit runs shutdown hooks, which can wait for the caller's thread.
            new Thread(() -> {
                try (Socket socket = connect()) {
                    send(socket, command);
                    reader(socket).readLine();
                } catch (Exception e) {
                    System.err.println("ServiceWrapper connection lost: " + e.getMessage());
                } finally {
                    System.exit(exitCode);
                }
            }, "ServiceWrapper shutdown").start();
        }
    }
}
