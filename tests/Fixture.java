import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import org.servicewrapper.WrapperManager;

public class Fixture {
    public static void main(String[] args) throws Exception {
        Path root = Path.of(System.getProperty("fixture.root"));
        String value = System.getProperty("fixture.value", "unset");
        long pid = ProcessHandle.current().pid();
        Files.writeString(root.resolve("events"), "start " + pid + " " + value + "\n",
                StandardOpenOption.CREATE, StandardOpenOption.APPEND);
        Runtime.getRuntime().addShutdownHook(new Thread(() -> {
            try {
                if ("hang".equals(System.getProperty("fixture.mode")))
                    Thread.sleep(60000);
                Files.writeString(root.resolve("events"), "stop " + pid + "\n", StandardOpenOption.APPEND);
            } catch (Exception e) {
                throw new RuntimeException(e);
            }
        }));
        System.out.println("property=" + value);
        System.out.println("home=" + System.getenv("HOME"));
        for (String arg : args)
            System.out.println("arg=" + arg);
        String mode = System.getProperty("fixture.mode", "service");
        if (mode.equals("once"))
            return;
        if (mode.equals("exit"))
            System.exit(7);
        if (mode.equals("crash"))
            Runtime.getRuntime().halt(9);
        if (mode.equals("logs")) {
            for (int i = 0; i < 2000; i++) {
                System.out.println("x".repeat(256));
                Thread.sleep(1);
            }
            return;
        }
        for (;;) {
            if (Files.deleteIfExists(root.resolve("restart"))) {
                if (Boolean.getBoolean("fixture.legacy_api"))
                    org.tanukisoftware.wrapper.WrapperManager.restartAndReturn();
                else
                    WrapperManager.restartAndReturn();
            }
            if (Files.deleteIfExists(root.resolve("stop"))) {
                if (Boolean.getBoolean("fixture.legacy_api"))
                    org.tanukisoftware.wrapper.WrapperManager.stopAndReturn(0);
                else
                    WrapperManager.stopAndReturn(0);
            }
            Thread.sleep(50);
        }
    }
}
