package demo;
import java.util.List;

/**
 * 实际响应字段使用 payload，插件必须尊重源码。
 * @author jhzhang
 * @since 2026-11-17
 */
public class Result<T> {
    /** 业务状态码 */
    private int code;
    /** 业务数据 */
    private T payload;
}

/**
 * 分页数据。
 * @author jhzhang
 * @since 2026-11-17
 */
class Page<T> {
    /** 当前页记录 */
    private List<T> rows;
    /** 总记录数 */
    private long total;
}
