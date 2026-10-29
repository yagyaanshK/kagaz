// The iPhone app's entry: the app itself is the Rust library (src/lib.rs),
// whose start_app runs UIKit's application loop and does not return.

extern void start_app(void);

int main(void) {
    start_app();
    return 0;
}
