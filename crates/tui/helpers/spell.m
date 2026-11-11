#import <AppKit/AppKit.h>
#import <stdio.h>
#import <string.h>

static void finish(void) {
    fputs(".\n", stdout);
    fflush(stdout);
}

static NSString *rest_of(const char *line) {
    const char *gap = strchr(line, ' ');
    if (gap == NULL || gap[1] == '\0') {
        return @"";
    }
    NSString *text = [NSString stringWithUTF8String:gap + 1];
    return text == nil ? @"" : text;
}

static BOOL starts(const char *line, const char *word) {
    size_t n = strlen(word);
    return strncmp(line, word, n) == 0 && (line[n] == '\0' || line[n] == ' ');
}

static void check(NSString *text) {
    if (text.length == 0) {
        finish();
        return;
    }
    NSSpellChecker *spell = [NSSpellChecker sharedSpellChecker];
    NSArray<NSTextCheckingResult *> *found = [spell checkString:text
                                                           range:NSMakeRange(0, text.length)
                                                           types:NSTextCheckingTypeSpelling
                                                         options:nil
                                          inSpellDocumentWithTag:0
                                                     orthography:NULL
                                                       wordCount:NULL];
    for (NSTextCheckingResult *result in found) {
        if (result.resultType != NSTextCheckingTypeSpelling) {
            continue;
        }
        printf("%lu %lu\n", (unsigned long)result.range.location, (unsigned long)result.range.length);
    }
    finish();
}

static NSString *language(NSSpellChecker *spell, NSString *text) {
    NSString *lang = [spell languageForWordRange:NSMakeRange(0, text.length) inString:text orthography:NULL];
    return lang == nil ? spell.language : lang;
}

static void complete(NSString *text) {
    if (text.length == 0) {
        finish();
        return;
    }
    NSSpellChecker *spell = [NSSpellChecker sharedSpellChecker];
    NSArray<NSString *> *words = [spell completionsForPartialWordRange:NSMakeRange(0, text.length)
                                                               inString:text
                                                               language:language(spell, text)
                                                  inSpellDocumentWithTag:0];
    NSString *best = words.firstObject;
    if (best != nil) {
        printf("%s\n", best.UTF8String);
    }
    finish();
}

static void correct(NSString *text) {
    if (text.length == 0) {
        finish();
        return;
    }
    NSSpellChecker *spell = [NSSpellChecker sharedSpellChecker];
    NSString *fixed = [spell correctionForWordRange:NSMakeRange(0, text.length)
                                            inString:text
                                            language:language(spell, text)
                               inSpellDocumentWithTag:0];
    if (fixed != nil && ![fixed isEqualToString:text]) {
        printf("%s\n", fixed.UTF8String);
    }
    finish();
}

static void guesses(NSString *text) {
    if (text.length == 0) {
        finish();
        return;
    }
    NSSpellChecker *spell = [NSSpellChecker sharedSpellChecker];
    NSArray<NSString *> *words = [spell guessesForWordRange:NSMakeRange(0, text.length)
                                                    inString:text
                                                    language:language(spell, text)
                                       inSpellDocumentWithTag:0];
    for (NSString *word in words) {
        printf("%s\n", word.UTF8String);
    }
    finish();
}

int main(void) {
    @autoreleasepool {
        char buf[8192];
        while (fgets(buf, sizeof buf, stdin) != NULL) {
            size_t n = strlen(buf);
            while (n > 0 && (buf[n - 1] == '\n' || buf[n - 1] == '\r')) {
                buf[--n] = '\0';
            }
            NSString *text = rest_of(buf);
            if (starts(buf, "check")) {
                check(text);
            } else if (starts(buf, "complete")) {
                complete(text);
            } else if (starts(buf, "correct")) {
                correct(text);
            } else if (starts(buf, "guesses")) {
                guesses(text);
            } else {
                finish();
            }
        }
    }
    return 0;
}
