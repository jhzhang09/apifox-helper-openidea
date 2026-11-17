package demo;
import java.util.Map;
import java.util.List;
import jakarta.validation.constraints.NotBlank;
import io.swagger.v3.oas.annotations.media.Schema;

/**
 * 用户资料。
 * @author jhzhang
 * @since 2026-11-17
 */
public class User {
    /** 用户姓名 */
    @NotBlank
    private String name;
    /** 居住地址 */
    private Address address;
    /** 分类联系地址 */
    private Map<String, List<Address>> contacts;
    /**
     * 地址。
     * @author jhzhang
     * @since 2026-11-17
     */
    static class Address {
        /** 城市名称 */
        @Schema(description = "城市名称", example = "北京")
        private String city;
        /** 地理坐标 */
        private Geo point;
    }
    /**
     * 地理坐标。
     * @author jhzhang
     * @since 2026-11-17
     */
    static class Geo {
        /** 纬度 */
        private double latitude;
        /** 经度 */
        private double longitude;
    }
}
