    fn make_thread(cwd: &str) -> ThreadMeta {
        ThreadMeta::new(cwd)
    }

    #[test]
    fn filter_keeps_matching_cwd() {
        let cwd = "/Users/alice/project";
        let threads = vec![
            make_thread(cwd),
            make_thread("/Users/alice/other"),
            make_thread(cwd),
        ];
        let filtered: Vec<_> = threads.into_iter().filter(|t| t.cwd == cwd).collect();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_returns_empty_when_no_match() {
        let cwd = "/Users/alice/project";
        let threads = vec![
            make_thread("/Users/alice/other"),
            make_thread("/Users/bob/project"),
        ];
        let filtered: Vec<_> = threads.into_iter().filter(|t| t.cwd == cwd).collect();
        assert!(filtered.is_empty());
    }

    #[test]
    fn filter_keeps_all_when_all_match() {
        let cwd = "/Users/alice/project";
        let threads = vec![make_thread(cwd), make_thread(cwd), make_thread(cwd)];
        let filtered: Vec<_> = threads.into_iter().filter(|t| t.cwd == cwd).collect();
        assert_eq!(filtered.len(), 3);
    }
