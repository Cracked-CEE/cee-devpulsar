import { describe, it, expect } from "vitest";
import fc from "fast-check";
import { GitLogService } from "../../../src/service/GitLogService";

// Placed under tests/unit (matching every other suite in this project and
// vitest.config.ts's `include: ["tests/unit/**/*.test.ts"]`) rather than
// src/service, so the suite actually gets picked up by `bun run test:unit`.

const SHA_RE = /^[a-f0-9]{40}$/;
const NUM_RUNS = 300; // keep CI runtime bounded while still covering ground

// ---------------------------------------------------------------------
// Arbitraries
// ---------------------------------------------------------------------

const shaArb = fc.stringMatching(SHA_RE);

/**
 * A single header-line-safe string: no newlines/CR (would break the parser's
 * line framing) and no angle brackets. Once a name/email itself contains
 * '<' or '>' the "Name <email>" grammar becomes ambiguous for any
 * line-oriented regex parser -- that's a known, accepted limitation, not
 * something this suite tries to fix. Adversarial angle-bracket content is
 * still covered separately by the "never throws" properties below.
 */
function safeLineArb(maxLength = 40) {
  return fc.string({ maxLength }).map((s) => s.replace(/[\r\n<>]/g, "_"));
}

function nonEmptySafeLineArb(maxLength = 40) {
  return safeLineArb(maxLength).filter((s) => s.trim().length > 0);
}

/**
 * Like nonEmptySafeLineArb, but additionally trimmed. `git log --format=fuller`
 * pads "Author:"/"Commit:" with variable spaces for column alignment, so the
 * parser must treat the separator as `\s+`; that makes a genuinely
 * leading/trailing-whitespace name fundamentally indistinguishable from
 * alignment padding. Round-trip fidelity properties use this arbitrary to
 * stay within the representable grammar.
 */
function trimmedNonEmptyLineArb(maxLength = 40) {
  return nonEmptySafeLineArb(maxLength).map((s) => s.trim());
}

// Deliberately adversarial text: control characters, astral/exotic unicode,
// binary-looking noise, and a few hand-picked traps.
const wildTextArb = fc.oneof(
  fc.string({ unit: "grapheme" }),
  fc.string({ unit: "binary-ascii" }),
  fc.constantFrom(
    "",
    "\0",
    "commit " + "f".repeat(40),
    "𝌆".repeat(20),
    "-by: <>",
    "Author: <>",
  ),
);

const dateArb = fc.oneof(
  fc
    .date({ min: new Date(0), max: new Date(2100, 0, 1), noInvalidDate: true })
    .map((d) => d.toISOString()),
  safeLineArb(30),
);

const trailerTypeArb = fc.oneof(
  fc.constantFrom(
    "Co-authored",
    "CO-AUTHORED",
    "reviewed",
    "Reviewed",
    "Tested",
    "TESTED",
    "Approved",
    "",
    "weird-Trailer",
  ),
  safeLineArb(15),
);

function trailerLineArb() {
  return fc
    .tuple(trailerTypeArb, nonEmptySafeLineArb(20), safeLineArb(20))
    .map(([type, name, email]) => `${type}-by: ${name} <${email}>`);
}

function indentMessage(message: string): string {
  return message
    .split("\n")
    .map((line) => (line.length ? `    ${line}` : "    "))
    .join("\n");
}

interface BlockOptions {
  sha: string;
  mergeParents?: string[];
  authorName: string;
  authorEmail: string;
  authorDate: string;
  committerName: string;
  committerEmail: string;
  committerDate: string;
  message: string;
}

/** Builds one `git log --format=fuller`-shaped commit block. */
function buildBlock(opts: BlockOptions): string {
  const lines = [`commit ${opts.sha}`];
  if (opts.mergeParents && opts.mergeParents.length > 0) {
    lines.push(`Merge: ${opts.mergeParents.join(" ")}`);
  }
  lines.push(`Author: ${opts.authorName} <${opts.authorEmail}>`);
  lines.push(`AuthorDate: ${opts.authorDate}`);
  lines.push(`Commit: ${opts.committerName} <${opts.committerEmail}>`);
  lines.push(`CommitDate: ${opts.committerDate}`);
  lines.push("");
  lines.push(indentMessage(opts.message));
  lines.push("");
  return lines.join("\n");
}

const validBlockArb = fc
  .record({
    sha: shaArb,
    authorName: trimmedNonEmptyLineArb(),
    authorEmail: fc.emailAddress(),
    authorDate: dateArb,
    committerName: trimmedNonEmptyLineArb(),
    committerEmail: fc.emailAddress(),
    committerDate: dateArb,
    messageLines: fc.array(safeLineArb(60), { maxLength: 6 }),
    trailers: fc.array(trailerLineArb(), { maxLength: 4 }),
  })
  .map((r) => ({
    ...r,
    message: [...r.messageLines, ...r.trailers].join("\n"),
  }));

// ---------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------

describe("GitLogService parser robustness (fuzz)", () => {
  describe("never throws / never produces a malformed sha", () => {
    it("does not throw on fully-random raw input", () => {
      fc.assert(
        fc.property(wildTextArb, (raw) => {
          expect(() => GitLogService.parseGitLog(raw)).not.toThrow();
        }),
        { numRuns: NUM_RUNS },
      );
    });

    it("never returns a commit with an undefined or malformed sha, for arbitrary mixes of noise and real commit headers", () => {
      fc.assert(
        fc.property(
          fc.array(fc.oneof(wildTextArb, shaArb), { maxLength: 20 }),
          (chunks) => {
            const raw = chunks
              .map((c) => (SHA_RE.test(c) ? `commit ${c}\n` : c))
              .join("\n");

            let commits;
            expect(() => {
              commits = GitLogService.parseGitLog(raw);
            }).not.toThrow();

            for (const commit of commits!) {
              expect(commit.sha).toBeDefined();
              expect(commit.sha).toMatch(SHA_RE);
            }
          },
        ),
        { numRuns: NUM_RUNS },
      );
    });

    it("does not throw when trailers, dates or messages are adversarial", () => {
      fc.assert(
        fc.property(
          shaArb,
          nonEmptySafeLineArb(),
          fc.emailAddress(),
          dateArb,
          fc.array(fc.oneof(wildTextArb, trailerLineArb()), { maxLength: 10 }),
          (sha, name, email, date, messageLines) => {
            const log = buildBlock({
              sha,
              authorName: name,
              authorEmail: email,
              authorDate: date,
              committerName: name,
              committerEmail: email,
              committerDate: date,
              message: messageLines.join("\n"),
            });

            let commits;
            expect(() => {
              commits = GitLogService.parseGitLog(log);
            }).not.toThrow();
            for (const commit of commits!) {
              expect(commit.sha).toMatch(SHA_RE);
              expect(commit.metadata).toBeDefined();
            }
          },
        ),
        { numRuns: NUM_RUNS },
      );
    });
  });

  describe("well-formed input round-trips without silent misparsing", () => {
    it("preserves sha, author, committer and message for a single well-formed commit", () => {
      fc.assert(
        fc.property(validBlockArb, (r) => {
          const log = buildBlock(r);
          const commits = GitLogService.parseGitLog(log);

          expect(commits).toHaveLength(1);
          expect(commits[0]!.sha).toBe(r.sha);
          expect(commits[0]!.author.name).toBe(r.authorName);
          expect(commits[0]!.author.email).toBe(r.authorEmail);
          expect(commits[0]!.committer.name).toBe(r.committerName);
          expect(commits[0]!.committer.email).toBe(r.committerEmail);
          expect(commits[0]!.message).toBe(r.message.trim());
        }),
        { numRuns: NUM_RUNS },
      );
    });

    it("parses N concatenated well-formed commits into exactly N commits", () => {
      fc.assert(
        fc.property(
          fc.array(validBlockArb, { minLength: 0, maxLength: 15 }),
          (blocks) => {
            const log = blocks.map(buildBlock).join("\n");
            const commits = GitLogService.parseGitLog(log);
            expect(commits).toHaveLength(blocks.length);
            const parsedShas = commits.map((c) => c.sha);
            const expectedShas = blocks.map((b) => b.sha);
            expect(parsedShas).toEqual(expectedShas);
          },
        ),
        { numRuns: NUM_RUNS },
      );
    });

    it("does not split a multi-parent merge commit, and ignores the Merge: line", () => {
      fc.assert(
        fc.property(
          validBlockArb,
          fc.array(fc.stringMatching(/^[a-f0-9]{7,40}$/), {
            minLength: 2,
            maxLength: 4,
          }),
          (r, mergeParents) => {
            const log = buildBlock({ ...r, mergeParents });
            const commits = GitLogService.parseGitLog(log);
            expect(commits).toHaveLength(1);
            expect(commits[0]!.sha).toBe(r.sha);
            expect(commits[0]!.author.name).toBe(r.authorName);
          },
        ),
        { numRuns: NUM_RUNS },
      );
    });

    it("does not split a commit whose message contains a properly-indented line resembling 'commit <sha>'", () => {
      fc.assert(
        fc.property(validBlockArb, shaArb, (r, embeddedSha) => {
          const message = [
            "This reverts commit",
            `commit ${embeddedSha}`,
            "which turned out to be faulty.",
          ].join("\n");
          const log = buildBlock({ ...r, message });
          const commits = GitLogService.parseGitLog(log);

          expect(commits).toHaveLength(1);
          expect(commits[0]!.sha).toBe(r.sha);
          expect(commits[0]!.message).toContain(`commit ${embeddedSha}`);
        }),
        { numRuns: NUM_RUNS },
      );
    });
  });

  describe("parseCommitMetadata does not catastrophically backtrack", () => {
    // No single input here is expected to be a genuine ReDoS trigger (the
    // trailer regex chains lazy quantifiers between fixed literals rather
    // than nesting them), but this pins down the expectation so a future
    // change to the pattern can't silently reintroduce exponential blowup.
    it("stays within a bounded time budget for large adversarial trailer-shaped input", () => {
      fc.assert(
        fc.property(fc.integer({ min: 1, max: 300 }), (repeat) => {
          const adversarialLine =
            "a".repeat(40) +
            "-by:" +
            " ".repeat(20) +
            "b".repeat(40) +
            " <" +
            "c".repeat(200); // deliberately never closed with '>'
          const message = Array(repeat).fill(adversarialLine).join("\n");
          const log = buildBlock({
            sha: "a".repeat(40),
            authorName: "A",
            authorEmail: "a@example.com",
            authorDate: "2026-01-01",
            committerName: "A",
            committerEmail: "a@example.com",
            committerDate: "2026-01-01",
            message,
          });

          const start = performance.now();
          GitLogService.parseGitLog(log);
          const elapsedMs = performance.now() - start;
          expect(elapsedMs).toBeLessThan(2000);
        }),
        { numRuns: 20 },
      );
    });

    it("stays within a bounded time budget for a single very large pathological message", () => {
      const adversarialLine =
        "x".repeat(60) + "-by: " + "y".repeat(60) + " <" + "z".repeat(500);
      const message = Array(5000).fill(adversarialLine).join("\n");
      const log = buildBlock({
        sha: "b".repeat(40),
        authorName: "A",
        authorEmail: "a@example.com",
        authorDate: "2026-01-01",
        committerName: "A",
        committerEmail: "a@example.com",
        committerDate: "2026-01-01",
        message,
      });

      const start = performance.now();
      const commits = GitLogService.parseGitLog(log);
      const elapsedMs = performance.now() - start;

      expect(elapsedMs).toBeLessThan(5000);
      expect(commits).toHaveLength(1);
    });
  });

  // -------------------------------------------------------------------
  // Regression corpus: real (author/email-anonymized) excerpts pulled
  // directly from `git log --format=fuller` on this repository, covering
  // edge cases already present in its own history.
  // -------------------------------------------------------------------
  describe("regression corpus: real anonymized excerpts from this repo's git log", () => {
    it("parses a real multi-parent merge commit and ignores the Merge: line", () => {
      const raw = [
        "commit be7e8896cc3d372231438669cf2292c5e93f669c",
        "Merge: 1061f86 af5c843",
        "Author:     Contributor One <contributor1@example.invalid>",
        "AuthorDate: Tue Sep 9 00:29:35 2025 +0200",
        "Commit:     Contributor One <contributor1@example.invalid>",
        "CommitDate: Tue Sep 9 00:29:35 2025 +0200",
        "",
        "    Merge remote-tracking branch 'origin/main'",
        "",
      ].join("\n");

      const commits = GitLogService.parseGitLog(raw);
      expect(commits).toHaveLength(1);
      expect(commits[0]!.sha).toBe("be7e8896cc3d372231438669cf2292c5e93f669c");
      // Regression guard: `git log --format=fuller` right-pads "Author:"/
      // "Commit:" with extra spaces to align with "AuthorDate:"/
      // "CommitDate:". Before the \s+ fix, this padding leaked into the
      // captured name as leading whitespace.
      expect(commits[0]!.author.name).toBe("Contributor One");
      expect(commits[0]!.committer.name).toBe("Contributor One");
    });

    it("parses a real commit with a Co-authored-by trailer", () => {
      const raw = [
        "commit 9c0346ab4b02d9e92dccb7cbdefc7f3a3650a2df",
        "Author:     Contributor Two <contributor2@example.invalid>",
        "AuthorDate: Tue May 12 22:51:22 2026 +0200",
        "Commit:     GitHub <noreply@github.com>",
        "CommitDate: Tue May 12 22:51:22 2026 +0200",
        "",
        "    ci: upload SBOMs with Filebase (#139)",
        "    ",
        "    Co-authored-by: Contributor Three <contributor3@example.invalid>",
        "",
      ].join("\n");

      const commits = GitLogService.parseGitLog(raw);
      expect(commits).toHaveLength(1);
      expect(commits[0]!.author.name).toBe("Contributor Two");
      expect(commits[0]!.message).toBe(
        "ci: upload SBOMs with Filebase (#139)\n\nCo-authored-by: Contributor Three <contributor3@example.invalid>",
      );
      expect(commits[0]!.metadata.coAuthoredBy).toHaveLength(1);
      expect(commits[0]!.metadata.coAuthoredBy[0]!.name).toBe(
        "Contributor Three",
      );
    });

    it("parses a real commit whose author name contains parentheses and spaces", () => {
      const raw = [
        "commit 728873dae817702c95e5417601127680837e8c9c",
        "Author:     Contributor Four (Alias) <contributor4@example.invalid>",
        "AuthorDate: Fri May 8 09:44:07 2026 +0100",
        "Commit:     GitHub <noreply@github.com>",
        "CommitDate: Fri May 8 10:44:07 2026 +0200",
        "",
        "    Feat: Add Function to remove a vote (#96)",
        "",
      ].join("\n");

      const commits = GitLogService.parseGitLog(raw);
      expect(commits).toHaveLength(1);
      expect(commits[0]!.author.name).toBe("Contributor Four (Alias)");
    });
  });
});
