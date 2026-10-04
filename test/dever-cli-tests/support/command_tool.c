#include <stdio.h>
#include <string.h>

extern int command_value(void);

int main(int argc, char **argv) {
    const unsigned char expected[] = {0, 255, 128, 10};
    unsigned char input[sizeof(expected)];
    if (argc != 3 || strcmp(argv[1], "a b") != 0 ||
        strcmp(argv[2], "$(literal);*") != 0 || command_value() != 42)
        return 90;
    if (fread(input, 1, sizeof(input), stdin) != sizeof(input) ||
        memcmp(input, expected, sizeof(input)) != 0 || getchar() != EOF)
        return 91;
    if (fwrite(input, 1, sizeof(input), stdout) != sizeof(input) ||
        fputs("ordinary command", stderr) == EOF)
        return 92;
    return 23;
}
