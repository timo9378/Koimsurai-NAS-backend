//! Property-based tests for security-critical functions
//! 
//! Uses proptest to generate random/malicious inputs to stress test:
//! - Path validation
//! - Filename sanitization
//! - Other security boundaries

use proptest::prelude::*;

// 直接從專案的 utils 模組引入真實的函式
// 這樣可以確保我們測試的是實際使用的程式碼，而非測試檔內的複製品
// Note: Cargo package name uses underscore, so we use Koimsurai_NAS
use Koimsurai_NAS::utils::versioning::{validate_path, sanitize_filename};

proptest! {
    /// Test that validate_path never allows path traversal
    #[test]
    fn path_never_allows_traversal(s in ".*") {
        // If the path contains "..", it should be rejected
        if s.contains("..") {
            prop_assert!(!validate_path(&s), "Path with '..' should be rejected: {}", s);
        }
    }
    
    /// Test that validate_path rejects absolute paths
    #[test]
    fn path_rejects_absolute(s in "/.*") {
        prop_assert!(!validate_path(&s), "Absolute path should be rejected: {}", s);
    }
    
    /// Test that validate_path rejects null bytes
    #[test]
    fn path_rejects_null_bytes(
        prefix in "[a-zA-Z0-9_/]{0,20}",
        suffix in "[a-zA-Z0-9_/.]{0,20}"
    ) {
        let path = format!("{}\0{}", prefix, suffix);
        prop_assert!(!validate_path(&path), "Path with null byte should be rejected");
    }
    
    /// Test that validate_path rejects Windows paths
    #[test]
    fn path_rejects_windows_drive(
        drive in "[A-Z]",
        path in "[a-zA-Z0-9_/]{0,30}"
    ) {
        let win_path = format!("{}:\\{}", drive, path);
        prop_assert!(!validate_path(&win_path), "Windows path should be rejected: {}", win_path);
    }
    
    /// Test that sanitize_filename produces safe output
    #[test]
    fn sanitize_produces_safe_filename(s in ".*") {
        let sanitized = sanitize_filename(&s);
        
        // Should not contain dangerous characters
        prop_assert!(!sanitized.contains('/'), "Should not contain /");
        prop_assert!(!sanitized.contains('\\'), "Should not contain \\");
        prop_assert!(!sanitized.contains(':'), "Should not contain :");
        prop_assert!(!sanitized.contains('\0'), "Should not contain null");
        prop_assert!(!sanitized.contains('*'), "Should not contain *");
        prop_assert!(!sanitized.contains('?'), "Should not contain ?");
        prop_assert!(!sanitized.contains('"'), "Should not contain \"");
        prop_assert!(!sanitized.contains('<'), "Should not contain <");
        prop_assert!(!sanitized.contains('>'), "Should not contain >");
        prop_assert!(!sanitized.contains('|'), "Should not contain |");
        
        // Should not start with a dot (hidden file)
        prop_assert!(!sanitized.starts_with('.'), "Should not start with .");
        
        // Should never be empty
        prop_assert!(!sanitized.is_empty(), "Should not be empty");
    }
    
    /// Test with adversarial filename patterns
    #[test]
    fn sanitize_handles_adversarial_filenames(
        prefix in "[.]{0,5}",
        dangerous in prop::sample::select(vec![
            "/", "\\", ":", "*", "?", "\"", "<", ">", "|", "\0",
            "../", "..\\", "/..", "\\..", "::$DATA"
        ]),
        suffix in "[a-zA-Z0-9]{0,10}"
    ) {
        let adversarial = format!("{}{}{}", prefix, dangerous, suffix);
        let sanitized = sanitize_filename(&adversarial);
        
        // All dangerous patterns should be sanitized
        prop_assert!(!sanitized.contains(".."), "Should not contain ..: {}", sanitized);
        prop_assert!(!sanitized.contains('/'), "Should not contain /: {}", sanitized);
        prop_assert!(!sanitized.contains('\\'), "Should not contain \\: {}", sanitized);
    }
    
    /// Test that extremely long paths are handled
    #[test]
    fn handles_long_paths(s in "[a-zA-Z0-9_]{1,1000}") {
        // Should not panic
        let _ = validate_path(&s);
    }
    
    /// Test that extremely long filenames are handled  
    #[test]
    fn handles_long_filenames(s in ".{1,1000}") {
        // Should not panic
        let _ = sanitize_filename(&s);
    }
    
    /// Test Unicode handling in paths
    #[test]
    fn handles_unicode_paths(s in "[\\p{L}\\p{N}_/]{1,100}") {
        // Unicode letters and numbers should generally be allowed
        // (unless they contain forbidden patterns)
        let result = validate_path(&s);
        // Just verify it doesn't panic
        let _ = result;
    }
    
    /// Test that valid paths are accepted
    #[test]
    fn accepts_valid_paths(
        segments in prop::collection::vec("[a-zA-Z0-9_-]{1,20}", 1..5)
    ) {
        let path = segments.join("/");
        prop_assert!(validate_path(&path), "Valid path should be accepted: {}", path);
    }
}

/// Additional targeted tests for edge cases
#[cfg(test)]
mod edge_cases {
    use super::*;
    
    #[test]
    fn test_unicode_normalization_attack() {
        // Some Unicode characters look like ".." but are different codepoints
        // U+FF0E is FULLWIDTH FULL STOP
        let sneaky_path = "folder/\u{FF0E}\u{FF0E}/etc/passwd";
        // This should be caught or the path should be normalized first
        // For now, just ensure it doesn't panic
        let _ = validate_path(sneaky_path);
    }
    
    #[test]
    fn test_url_encoded_traversal() {
        // %2e%2e%2f = ../
        // This test assumes the input has already been URL decoded
        // If your web framework doesn't auto-decode, you need to handle this
        let encoded = "%2e%2e%2fpasswd";
        // After decoding this would be "../passwd"
        // The raw encoded form might pass validation
        let _ = validate_path(encoded);
    }
    
    #[test]
    fn test_double_url_encoding() {
        // %252e%252e%252f = %2e%2e%2f (after one decode) = ../ (after second decode)
        let double_encoded = "%252e%252e%252f";
        let _ = validate_path(double_encoded);
    }
    
    #[test]
    fn test_ntfs_alternate_data_streams() {
        // Windows NTFS alternate data streams
        let ads = "file.txt:$DATA";
        let sanitized = sanitize_filename(ads);
        assert!(!sanitized.contains(':'), "Should remove NTFS ADS marker");
    }
    
    #[test]
    fn test_windows_reserved_names() {
        // Windows reserved device names
        let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"];
        for name in reserved {
            let sanitized = sanitize_filename(name);
            // Should at least not panic
            assert!(!sanitized.is_empty());
        }
    }
    
    #[test]
    fn test_path_with_only_dots() {
        assert!(!validate_path("."));
        assert!(!validate_path(".."));
        assert!(!validate_path("..."));
        assert!(!validate_path("folder/."));  // Current dir reference should be rejected
        assert!(!validate_path("./folder"));  // Starting with current dir should be rejected
    }
}
