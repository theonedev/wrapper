package example;

import java.util.Arrays;
import java.util.concurrent.CountDownLatch;

/** A normal Java application: no dependency on Wrapper or a service framework. */
public final class HelloService {
    public static void main(String[] args) throws InterruptedException {
        CountDownLatch stopped = new CountDownLatch(1);
        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            // Close listeners, flush data, and release application resources here.
            System.out.println("Hello service stopped: shutdown hook completed");
            stopped.countDown();
        }, "Hello service shutdown"));

        System.out.println("Hello service started");
        for (String arg : args)
            System.out.println("Argument: " + arg);
        System.out.println("Greeting: " + System.getProperty("hello.greeting", "Hello from Java"));
        if (!Arrays.asList(args).contains("--once"))
            stopped.await(); // Keep the application alive until a service stop or Ctrl+C.
    }
}
