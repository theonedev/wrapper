import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import io.onedev.wrapper.WrapperManager;

public class Pid1Fixture {
    private static final Path PIDS = Path.of("/tmp/orphans");

    private static void awaitReaping(List<Long> pids) throws Exception {
        long deadline = System.nanoTime() + Duration.ofSeconds(5).toNanos();
        for (long pid : pids) {
            while (Files.exists(Path.of("/proc/" + pid))) {
                if (System.nanoTime() >= deadline)
                    throw new AssertionError("Orphan was not reaped: "
                            + Files.readString(Path.of("/proc/" + pid + "/status")));
                Thread.sleep(25);
            }
        }
    }

    public static void main(String[] args) throws Exception {
        if (ProcessHandle.current().parent().orElseThrow().pid() != 1)
            throw new AssertionError("Wrapper must be PID 1");
        if (Files.exists(PIDS)) {
            awaitReaping(Files.readAllLines(PIDS).stream().map(Long::valueOf).toList());
            System.out.println("Restart preserved; orphans reaped");
            return;
        }
        List<Long> pids = new ArrayList<>();
        for (int i = 0; i < 5; i++) {
            Process parent = new ProcessBuilder("sh", "-c",
                    "setsid sh -c 'sleep 0.3; exit 23' </dev/null >/dev/null 2>&1 & echo $!").start();
            long pid = Long.parseLong(new String(parent.getInputStream().readAllBytes()).trim());
            if (parent.waitFor() != 0)
                throw new AssertionError("Orphan parent failed");
            pids.add(pid);
        }
        awaitReaping(pids);
        System.out.println("All five adopted children reaped");
        String mode = args[0];
        if (mode.equals("restart")) {
            Process parent = new ProcessBuilder("sh", "-c",
                    "setsid sleep 1 </dev/null >/dev/null 2>&1 & echo $!").start();
            Files.write(PIDS, parent.getInputStream().readAllBytes());
            if (parent.waitFor() != 0)
                throw new AssertionError("Orphan parent failed");
            WrapperManager.restartAndReturn();
        } else if (mode.equals("signal")) {
            Runtime.getRuntime().addShutdownHook(new Thread(() -> System.out.println("Shutdown hook ran")));
            System.out.println("Ready for signal");
            Thread.sleep(60000);
        } else if (mode.equals("crash")) {
            Runtime.getRuntime().halt(9);
        } else {
            System.exit(Integer.parseInt(mode));
        }
    }
}
